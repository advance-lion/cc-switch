//! Lifecycle management for registered desktop applications.
//!
//! Desktop packages are deliberately separate from CLI tools. Every operation
//! resolves through this fixed registry, so the renderer can never provide an
//! executable path, package identifier, download URL, or arbitrary command.

use crate::database::LifecycleJobRecord;
use crate::store::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::State;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};
use uuid::Uuid;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Clone, Copy)]
struct DesktopAppManifest {
    id: &'static str,
    display_name: &'static str,
    windows_package_name: &'static str,
    windows_display_name: &'static str,
    windows_app_id_suffix: &'static str,
    winget_id: &'static str,
    winget_source: &'static str,
    macos_bundle_path: &'static str,
    macos_app_name: &'static str,
    macos_cask: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct RegisteredDesktopAssistantInstall {
    pub app_id: String,
    pub display_name: String,
    pub version: String,
    pub official_source: String,
}

const CODEX_DESKTOP: DesktopAppManifest = DesktopAppManifest {
    id: "codex-desktop",
    display_name: "Codex Desktop",
    windows_package_name: "OpenAI.Codex",
    windows_display_name: "Codex",
    windows_app_id_suffix: "App",
    // Official Microsoft Store product id from the OpenAI Codex Windows page.
    winget_id: "9PLM9XGG6VKS",
    winget_source: "msstore",
    macos_bundle_path: "/Applications/Codex.app",
    macos_app_name: "Codex",
    macos_cask: "codex-app",
};

const CLAUDE_DESKTOP: DesktopAppManifest = DesktopAppManifest {
    id: "claude-desktop",
    display_name: "Claude Desktop",
    windows_package_name: "Claude",
    windows_display_name: "Claude",
    windows_app_id_suffix: "Claude",
    winget_id: "Anthropic.Claude",
    winget_source: "winget",
    macos_bundle_path: "/Applications/Claude.app",
    macos_app_name: "Claude",
    macos_cask: "claude",
};

fn manifest(app: &str) -> Result<DesktopAppManifest, String> {
    match app {
        "codex-desktop" => Ok(CODEX_DESKTOP),
        "claude-desktop" => Ok(CLAUDE_DESKTOP),
        _ => Err(format!("Unsupported desktop application: {app}")),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopAppStatus {
    pub id: String,
    pub display_name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub latest_version: Option<String>,
    pub path: Option<String>,
    #[serde(default)]
    pub launch_target: Option<String>,
    pub package_identity: Option<String>,
    pub installation_source: String,
    pub can_install: bool,
    pub can_update: bool,
    pub can_uninstall: bool,
    pub can_launch: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AppxRecord {
    kind: String,
    version: String,
    package_full_name: Option<String>,
    package_family_name: Option<String>,
    install_location: String,
    launch_target: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum DesktopLifecycleAction {
    Install,
    Update,
    Uninstall,
}

impl DesktopLifecycleAction {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "install" => Ok(Self::Install),
            "update" => Ok(Self::Update),
            "uninstall" => Ok(Self::Uninstall),
            _ => Err(format!("Unsupported desktop lifecycle action: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Uninstall => "uninstall",
        }
    }
}

#[derive(Default)]
pub struct DesktopLifecycleOperationState {
    locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
    cancellations: RwLock<HashMap<String, Arc<AtomicBool>>>,
}

impl DesktopLifecycleOperationState {
    async fn lock(&self, app_id: &str) -> OwnedMutexGuard<()> {
        let lock = if let Some(lock) = self.locks.read().await.get(app_id).cloned() {
            lock
        } else {
            let mut locks = self.locks.write().await;
            locks
                .entry(app_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        lock.lock_owned().await
    }

    async fn register_job(&self, job_id: &str) -> Arc<AtomicBool> {
        let cancellation = Arc::new(AtomicBool::new(false));
        self.cancellations
            .write()
            .await
            .insert(job_id.to_string(), cancellation.clone());
        cancellation
    }

    async fn finish_job(&self, job_id: &str) {
        self.cancellations.write().await.remove(job_id);
    }

    async fn cancel_job(&self, job_id: &str) -> bool {
        if let Some(cancellation) = self.cancellations.read().await.get(job_id) {
            cancellation.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopLifecycleJob {
    pub id: String,
    pub app_id: String,
    pub component: String,
    pub action: String,
    pub state: String,
    pub pre_probe: Option<DesktopAppStatus>,
    pub post_probe: Option<DesktopAppStatus>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn job_record(job: &DesktopLifecycleJob) -> Result<LifecycleJobRecord, String> {
    Ok(LifecycleJobRecord {
        id: job.id.clone(),
        app_id: job.app_id.clone(),
        component: job.component.clone(),
        action: job.action.clone(),
        state: job.state.clone(),
        plan_json: serde_json::to_string(&serde_json::json!({
            "appId": job.app_id,
            "component": job.component,
            "action": job.action,
            "source": "registered-manifest",
        }))
        .map_err(|error| error.to_string())?,
        pre_probe_json: job
            .pre_probe
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| error.to_string())?,
        post_probe_json: job
            .post_probe
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| error.to_string())?,
        error_code: job.error_code.clone(),
        error_message: job.error_message.clone(),
        created_at: job.created_at,
        started_at: job.started_at,
        completed_at: job.completed_at,
    })
}

fn job_from_record(record: LifecycleJobRecord) -> Result<DesktopLifecycleJob, String> {
    Ok(DesktopLifecycleJob {
        id: record.id,
        app_id: record.app_id,
        component: record.component,
        action: record.action,
        state: record.state,
        pre_probe: record
            .pre_probe_json
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| format!("Invalid lifecycle pre-probe: {error}"))?,
        post_probe: record
            .post_probe_json
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| format!("Invalid lifecycle post-probe: {error}"))?,
        error_code: record.error_code,
        error_message: record.error_message,
        created_at: record.created_at,
        started_at: record.started_at,
        completed_at: record.completed_at,
    })
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn command_output(command: &mut Command, label: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = command
        .output()
        .map_err(|error| format!("Failed to start {label}: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(format!(
            "{label} failed with exit code {}{}",
            output.status.code().unwrap_or(-1),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ));
    }
    Ok(stdout)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn command_output_cancellable(
    command: &mut Command,
    label: &str,
    cancellation: &AtomicBool,
) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to start {label}: {error}"))?;
    let status = loop {
        if cancellation.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("JOB_CANCELLED".to_string());
        }
        match child
            .try_wait()
            .map_err(|error| format!("Failed while waiting for {label}: {error}"))?
        {
            Some(status) => break status,
            None => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let stdout = stdout.trim().to_string();
    let stderr = stderr.trim().to_string();
    if !status.success() {
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(format!(
            "{label} failed with exit code {}{}",
            status.code().unwrap_or(-1),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ));
    }
    Ok(stdout)
}

#[cfg(target_os = "windows")]
fn powershell_output(script: &str, label: &str) -> Result<String, String> {
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    command_output(&mut command, label)
}

#[cfg(target_os = "windows")]
fn detect_desktop_app(manifest: DesktopAppManifest) -> Result<DesktopAppStatus, String> {
    let script = format!(
        r#"[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$pkg = Get-AppxPackage -Name '{}' | Sort-Object Version -Descending | Select-Object -First 1
if ($null -ne $pkg) {{
  [pscustomobject]@{{
    kind = 'appx'
    version = $pkg.Version.ToString()
    package_full_name = $pkg.PackageFullName
    package_family_name = $pkg.PackageFamilyName
    install_location = $pkg.InstallLocation
    launch_target = $null
  }} | ConvertTo-Json -Compress
  exit 0
}}
$uninstallRoots = @(
  'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
)
$entry = Get-ItemProperty $uninstallRoots -ErrorAction SilentlyContinue |
  Where-Object {{ $_.DisplayName -eq '{}' }} |
  Sort-Object DisplayVersion -Descending | Select-Object -First 1
if ($null -eq $entry) {{ Write-Output 'null'; exit 0 }}
$launch = $entry.DisplayIcon
if ($launch) {{ $launch = $launch.Trim('"').Split(',')[0] }}
[pscustomobject]@{{
  kind = 'win32'
  version = [string]$entry.DisplayVersion
  package_full_name = $null
  package_family_name = $null
  install_location = [string]$entry.InstallLocation
  launch_target = $launch
}} | ConvertTo-Json -Compress"#,
        manifest.windows_package_name, manifest.windows_display_name
    );
    let output = powershell_output(&script, "desktop application detection")?;
    if output.trim().is_empty() || output.trim() == "null" {
        return Ok(DesktopAppStatus {
            id: manifest.id.to_string(),
            display_name: manifest.display_name.to_string(),
            installed: false,
            version: None,
            latest_version: None,
            path: None,
            launch_target: None,
            package_identity: None,
            installation_source: "not_installed".to_string(),
            can_install: true,
            can_update: false,
            can_uninstall: false,
            can_launch: false,
            reason: None,
        });
    }

    let record: AppxRecord = serde_json::from_str(&output)
        .map_err(|error| format!("Invalid desktop package metadata: {error}"))?;
    if record.kind == "appx" {
        let valid_identity = record
            .package_full_name
            .as_deref()
            .is_some_and(|value| value.starts_with(manifest.windows_package_name));
        let valid_family = record
            .package_family_name
            .as_deref()
            .is_some_and(|value| value.starts_with(manifest.windows_package_name));
        if !valid_identity || !valid_family {
            return Err(format!(
                "Detected package identity does not match {}",
                manifest.display_name
            ));
        }
    }

    Ok(DesktopAppStatus {
        id: manifest.id.to_string(),
        display_name: manifest.display_name.to_string(),
        installed: true,
        version: Some(record.version),
        latest_version: None,
        path: Some(record.install_location),
        launch_target: record.launch_target,
        package_identity: record.package_full_name,
        installation_source: if record.kind == "win32" {
            "official_exe".to_string()
        } else if manifest.id == "codex-desktop" {
            "microsoft_store".to_string()
        } else {
            "official_appx".to_string()
        },
        can_install: false,
        can_update: true,
        can_uninstall: true,
        can_launch: true,
        reason: None,
    })
}

#[cfg(target_os = "macos")]
fn detect_desktop_app(manifest: DesktopAppManifest) -> Result<DesktopAppStatus, String> {
    let path = std::path::Path::new(manifest.macos_bundle_path);
    let installed = path.is_dir();
    let version = if installed {
        Command::new("defaults")
            .args([
                "read",
                &format!("{}/Contents/Info", manifest.macos_bundle_path),
                "CFBundleShortVersionString",
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty())
    } else {
        None
    };
    let homebrew_available = Command::new("brew")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    Ok(DesktopAppStatus {
        id: manifest.id.to_string(),
        display_name: manifest.display_name.to_string(),
        installed,
        version,
        latest_version: None,
        path: installed.then(|| manifest.macos_bundle_path.to_string()),
        launch_target: installed.then(|| manifest.macos_bundle_path.to_string()),
        package_identity: None,
        installation_source: if installed {
            "application_bundle"
        } else {
            "not_installed"
        }
        .to_string(),
        can_install: !installed && homebrew_available,
        can_update: installed && homebrew_available,
        can_uninstall: installed && homebrew_available,
        can_launch: installed,
        reason: (!homebrew_available)
            .then(|| "需要 Homebrew 才能自动安装、更新或卸载；仍可启动现有应用".to_string()),
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn detect_desktop_app(manifest: DesktopAppManifest) -> Result<DesktopAppStatus, String> {
    Ok(DesktopAppStatus {
        id: manifest.id.to_string(),
        display_name: manifest.display_name.to_string(),
        installed: false,
        version: None,
        latest_version: None,
        path: None,
        launch_target: None,
        package_identity: None,
        installation_source: "unsupported_platform".to_string(),
        can_install: false,
        can_update: false,
        can_uninstall: false,
        can_launch: false,
        reason: Some(
            "This desktop application is not supported on the current platform.".to_string(),
        ),
    })
}

fn numeric_version_parts(value: &str) -> Vec<u64> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u64>().ok())
        .collect()
}

fn version_is_newer(latest: &str, current: &str) -> bool {
    let mut latest_parts = numeric_version_parts(latest);
    let mut current_parts = numeric_version_parts(current);
    let length = latest_parts.len().max(current_parts.len());
    latest_parts.resize(length, 0);
    current_parts.resize(length, 0);
    latest_parts > current_parts
}

fn verification_satisfied(
    action: DesktopLifecycleAction,
    before: &DesktopAppStatus,
    after: &DesktopAppStatus,
) -> bool {
    match action {
        DesktopLifecycleAction::Install => after.installed,
        DesktopLifecycleAction::Update => {
            after.installed
                && (before.version.is_none()
                    || after.version.as_deref() != before.version.as_deref())
        }
        DesktopLifecycleAction::Uninstall => !after.installed,
    }
}

fn detect_after_action(
    manifest: DesktopAppManifest,
    action: DesktopLifecycleAction,
    before: &DesktopAppStatus,
) -> Result<DesktopAppStatus, String> {
    let mut last = detect_desktop_app(manifest)?;
    if verification_satisfied(action, before, &last) {
        return Ok(last);
    }
    // Store/Appx registration can trail the installer process. Poll briefly
    // before declaring verification failure, while keeping success tied to a
    // real post-action probe.
    for _ in 0..29 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        last = detect_desktop_app(manifest)?;
        if verification_satisfied(action, before, &last) {
            return Ok(last);
        }
    }
    Ok(last)
}

#[cfg(target_os = "windows")]
fn latest_winget_version(manifest: DesktopAppManifest) -> Result<String, String> {
    let mut command = Command::new("winget.exe");
    command.args([
        "show",
        "--id",
        manifest.winget_id,
        "--exact",
        "--source",
        manifest.winget_source,
        "--accept-source-agreements",
        "--disable-interactivity",
    ]);
    let output = command_output(&mut command, "desktop update lookup")?;
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed
            .strip_prefix("Version:")
            .or_else(|| trimmed.strip_prefix("版本:"))
            .or_else(|| trimmed.strip_prefix("版本："))
        {
            let version = value.trim();
            if !version.is_empty() {
                return Ok(version.to_string());
            }
        }
    }
    Err(format!(
        "Could not determine the latest {} version from winget",
        manifest.display_name
    ))
}

#[cfg(target_os = "macos")]
fn latest_winget_version(manifest: DesktopAppManifest) -> Result<String, String> {
    let mut command = Command::new("brew");
    command.args(["info", "--cask", "--json=v2", manifest.macos_cask]);
    let output = command_output(&mut command, "desktop update lookup")?;
    let value: serde_json::Value = serde_json::from_str(&output)
        .map_err(|error| format!("Invalid Homebrew metadata: {error}"))?;
    value
        .pointer("/casks/0/version")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "Could not determine the latest {} version",
                manifest.display_name
            )
        })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn latest_winget_version(manifest: DesktopAppManifest) -> Result<String, String> {
    Err(format!(
        "Automatic {} update checks are currently supported on Windows only",
        manifest.display_name
    ))
}

#[cfg(target_os = "macos")]
fn run_macos_action(
    manifest: DesktopAppManifest,
    action: DesktopLifecycleAction,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    let verb = match action {
        DesktopLifecycleAction::Install => "install",
        DesktopLifecycleAction::Update => "upgrade",
        DesktopLifecycleAction::Uninstall => "uninstall",
    };
    let mut command = Command::new("brew");
    command.args([verb, "--cask", manifest.macos_cask]);
    command_output_cancellable(&mut command, "desktop lifecycle action", cancellation).map(|_| ())
}

#[cfg(target_os = "windows")]
fn winget_action_command(
    manifest: DesktopAppManifest,
    action: DesktopLifecycleAction,
) -> Result<Command, String> {
    let verb = match action {
        DesktopLifecycleAction::Install => "install",
        DesktopLifecycleAction::Update => "upgrade",
        DesktopLifecycleAction::Uninstall => {
            return Err("Desktop Appx removal uses the package identity verifier".to_string())
        }
    };
    let mut command = Command::new("winget.exe");
    command.args([
        verb,
        "--id",
        manifest.winget_id,
        "--exact",
        "--source",
        manifest.winget_source,
        "--accept-package-agreements",
        "--accept-source-agreements",
        "--silent",
        "--disable-interactivity",
    ]);
    Ok(command)
}

#[cfg(target_os = "windows")]
fn run_winget_action(
    manifest: DesktopAppManifest,
    action: DesktopLifecycleAction,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    let mut command = winget_action_command(manifest, action)?;
    command_output_cancellable(&mut command, "desktop lifecycle action", cancellation).map(|_| ())
}

#[cfg(target_os = "windows")]
fn run_winget_uninstall(
    manifest: DesktopAppManifest,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    let mut command = Command::new("winget.exe");
    command.args([
        "uninstall",
        "--id",
        manifest.winget_id,
        "--exact",
        "--source",
        manifest.winget_source,
        "--silent",
        "--disable-interactivity",
    ]);
    command_output_cancellable(&mut command, "desktop application uninstall", cancellation)
        .map(|_| ())
}

pub(crate) fn plan_registered_desktop_install(
    app: &str,
    requested_version: &str,
    custom_install_location: bool,
) -> Result<RegisteredDesktopAssistantInstall, String> {
    let manifest = manifest(app)?;
    if custom_install_location {
        return Err(format!(
            "{} 由系统安装器管理，不支持自定义安装位置",
            manifest.display_name
        ));
    }
    let version = requested_version.trim();
    if !version.is_empty() && !matches!(version, "stable" | "latest") {
        return Err(format!(
            "{} 的桌面安装器不支持指定版本",
            manifest.display_name
        ));
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    return Err(format!(
        "{} 的 AI 辅助桌面安装当前不支持此平台",
        manifest.display_name
    ));
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    Ok(RegisteredDesktopAssistantInstall {
        app_id: manifest.id.to_string(),
        display_name: manifest.display_name.to_string(),
        version: "latest".to_string(),
        official_source: {
            #[cfg(target_os = "windows")]
            {
                if manifest.id == "codex-desktop" {
                    "https://apps.microsoft.com/detail/9PLM9XGG6VKS".to_string()
                } else {
                    "https://claude.ai/download".to_string()
                }
            }
            #[cfg(target_os = "macos")]
            {
                format!("https://formulae.brew.sh/cask/{}", manifest.macos_cask)
            }
        },
    })
}

pub(crate) fn spawn_registered_desktop_install(
    install: &RegisteredDesktopAssistantInstall,
) -> Result<Child, String> {
    let manifest = manifest(&install.app_id)?;
    #[cfg(target_os = "windows")]
    {
        let mut command = winget_action_command(manifest, DesktopLifecycleAction::Install)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW);
        return command
            .spawn()
            .map_err(|error| format!("无法启动 {} 安装器: {error}", manifest.display_name));
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("brew");
        command
            .args(["install", "--cask", manifest.macos_cask])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        return command
            .spawn()
            .map_err(|error| format!("无法启动 {} 安装器: {error}", manifest.display_name));
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    Err(format!(
        "{} 的 AI 辅助桌面安装当前不支持此平台",
        manifest.display_name
    ))
}

pub(crate) fn verify_registered_desktop_install(
    install: &RegisteredDesktopAssistantInstall,
) -> Result<String, String> {
    let manifest = manifest(&install.app_id)?;
    let status = detect_desktop_app(manifest)?;
    if !status.installed {
        return Err(format!(
            "{} 安装器已结束，但未检测到桌面应用",
            install.display_name
        ));
    }
    status
        .version
        .ok_or_else(|| format!("{} 已安装，但无法读取版本", install.display_name))
}

#[cfg(target_os = "windows")]
fn uninstall_appx(
    manifest: DesktopAppManifest,
    package_identity: &str,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    let valid_identity = package_identity
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character));
    if !valid_identity || !package_identity.starts_with(manifest.windows_package_name) {
        return Err("Refusing to remove an unverified desktop package identity".to_string());
    }
    let script = format!(
        "$pkg = Get-AppxPackage -Name '{}'; if ($null -eq $pkg) {{ exit 0 }}; $pkg | Where-Object {{ $_.PackageFullName -eq '{}' }} | Remove-AppxPackage -ErrorAction Stop",
        manifest.windows_package_name, package_identity
    );
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    command_output_cancellable(&mut command, "desktop application uninstall", cancellation)
        .map(|_| ())
}

#[tauri::command]
pub async fn get_desktop_app_status(app: String) -> Result<DesktopAppStatus, String> {
    let manifest = manifest(&app)?;
    tokio::task::spawn_blocking(move || detect_desktop_app(manifest))
        .await
        .map_err(|error| format!("Desktop detection task failed: {error}"))?
}

#[tauri::command]
pub async fn check_desktop_app_updates(app: String) -> Result<DesktopAppStatus, String> {
    let manifest = manifest(&app)?;
    tokio::task::spawn_blocking(move || {
        let mut status = detect_desktop_app(manifest)?;
        let latest = latest_winget_version(manifest)?;
        status.latest_version = Some(latest);
        Ok(status)
    })
    .await
    .map_err(|error| format!("Desktop update check task failed: {error}"))?
}

#[tauri::command]
pub async fn run_desktop_app_lifecycle_action(
    state: State<'_, AppState>,
    operations: State<'_, DesktopLifecycleOperationState>,
    app: String,
    action: String,
    #[allow(non_snake_case)] jobId: Option<String>,
) -> Result<DesktopAppStatus, String> {
    let manifest = manifest(&app)?;
    let action = DesktopLifecycleAction::parse(&action)?;
    let _guard = operations.lock(&app).await;
    let db = state.db.clone();
    let job_id = jobId.unwrap_or_else(|| Uuid::new_v4().to_string());
    if let Some(existing) = state
        .db
        .get_lifecycle_job(&job_id)
        .map_err(|error| error.to_string())?
    {
        let existing = job_from_record(existing)?;
        if let Some(status) = existing.post_probe {
            return Ok(status);
        }
        return Err(existing
            .error_message
            .unwrap_or_else(|| "相同请求仍在处理中，请稍后重试".to_string()));
    }
    let cancellation = operations.register_job(&job_id).await;
    let result = tokio::task::spawn_blocking({
        let job_id = job_id.clone();
        move || run_lifecycle_job(db, job_id, manifest, action, cancellation)
    })
    .await
    .map_err(|error| format!("Desktop lifecycle task failed: {error}"))?;
    operations.finish_job(&job_id).await;
    result
}

fn save_job(db: &crate::database::Database, job: &DesktopLifecycleJob) -> Result<(), String> {
    db.save_lifecycle_job(&job_record(job)?)
        .map_err(|error| error.to_string())
}

fn fail_job(
    db: &crate::database::Database,
    job: &mut DesktopLifecycleJob,
    code: &str,
    message: String,
) -> String {
    job.state = "failed".to_string();
    job.error_code = Some(code.to_string());
    job.error_message = Some(message.clone());
    job.completed_at = Some(now());
    if let Err(error) = save_job(db, job) {
        return format!("{message} (and failed to persist job: {error})");
    }
    message
}

fn run_lifecycle_job(
    db: Arc<crate::database::Database>,
    job_id: String,
    manifest: DesktopAppManifest,
    action: DesktopLifecycleAction,
    cancellation: Arc<AtomicBool>,
) -> Result<DesktopAppStatus, String> {
    let created_at = now();
    let mut job = DesktopLifecycleJob {
        id: job_id,
        app_id: manifest.id.to_string(),
        component: "desktop".to_string(),
        action: action.as_str().to_string(),
        state: "queued".to_string(),
        pre_probe: None,
        post_probe: None,
        error_code: None,
        error_message: None,
        created_at,
        started_at: None,
        completed_at: None,
    };
    save_job(&db, &job)?;
    job.state = "running".to_string();
    job.started_at = Some(now());
    save_job(&db, &job)?;

    let before = match detect_desktop_app(manifest) {
        Ok(status) => status,
        Err(error) => return Err(fail_job(&db, &mut job, "APP_PROBE_FAILED", error)),
    };
    job.pre_probe = Some(before.clone());
    if let Err(error) = save_job(&db, &job) {
        return Err(fail_job(&db, &mut job, "JOB_PERSIST_FAILED", error));
    }

    let execute_result = match action {
        DesktopLifecycleAction::Install => {
            if before.installed {
                Err(format!("{} is already installed", manifest.display_name))
            } else {
                #[cfg(target_os = "windows")]
                {
                    run_winget_action(manifest, action, &cancellation)
                }
                #[cfg(target_os = "macos")]
                {
                    run_macos_action(manifest, action, &cancellation)
                }
                #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                {
                    Err(
                        "Automatic desktop installation is not supported on this platform"
                            .to_string(),
                    )
                }
            }
        }
        DesktopLifecycleAction::Update => {
            if !before.installed {
                Err(format!("{} is not installed", manifest.display_name))
            } else {
                match latest_winget_version(manifest) {
                    Ok(latest) => {
                        let current = before.version.as_deref().unwrap_or_default();
                        if !version_is_newer(&latest, current) {
                            let mut current_status = before.clone();
                            current_status.latest_version = Some(latest);
                            job.post_probe = Some(current_status.clone());
                            job.state = "succeeded".to_string();
                            job.completed_at = Some(now());
                            save_job(&db, &job)?;
                            return Ok(current_status);
                        }
                        #[cfg(target_os = "windows")]
                        {
                            run_winget_action(manifest, action, &cancellation)
                        }
                        #[cfg(target_os = "macos")]
                        {
                            run_macos_action(manifest, action, &cancellation)
                        }
                        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                        {
                            Err(
                                "Automatic desktop updates are not supported on this platform"
                                    .to_string(),
                            )
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
        DesktopLifecycleAction::Uninstall => {
            if !before.installed {
                job.post_probe = Some(before.clone());
                job.state = "succeeded".to_string();
                job.completed_at = Some(now());
                save_job(&db, &job)?;
                return Ok(before);
            }
            #[cfg(target_os = "windows")]
            {
                if let Some(identity) = before.package_identity.as_deref() {
                    uninstall_appx(manifest, identity, &cancellation)
                } else {
                    run_winget_uninstall(manifest, &cancellation)
                }
            }
            #[cfg(target_os = "macos")]
            {
                run_macos_action(manifest, action, &cancellation)
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                Err("Automatic desktop uninstall is not supported on this platform".to_string())
            }
        }
    };
    if let Err(error) = execute_result {
        let code = if error == "JOB_CANCELLED" {
            "JOB_CANCELLED"
        } else {
            "LIFECYCLE_EXECUTION_FAILED"
        };
        if code == "JOB_CANCELLED" {
            job.state = "cancelled".to_string();
            job.error_code = Some(code.to_string());
            job.error_message = Some("操作已停止".to_string());
            job.completed_at = Some(now());
            save_job(&db, &job)?;
            return Err("操作已停止".to_string());
        }
        return Err(fail_job(&db, &mut job, code, error));
    }

    job.state = "verifying".to_string();
    save_job(&db, &job)?;
    let after = match detect_after_action(manifest, action, &before) {
        Ok(status) => status,
        Err(error) => {
            return Err(fail_job(
                &db,
                &mut job,
                "POST_INSTALL_VERIFICATION_FAILED",
                error,
            ))
        }
    };
    job.post_probe = Some(after.clone());
    let verification_error = match action {
        DesktopLifecycleAction::Install if !after.installed => Some(format!(
            "{} installer completed but the application was not detected",
            manifest.display_name
        )),
        DesktopLifecycleAction::Update
            if !after.installed
                || before.version.is_some()
                    && after.version.as_deref() == before.version.as_deref() =>
        {
            Some(format!(
                "{} update completed but the installed version did not change",
                manifest.display_name
            ))
        }
        DesktopLifecycleAction::Uninstall if after.installed => Some(format!(
            "{} uninstall completed but the package is still installed",
            manifest.display_name
        )),
        _ => None,
    };
    if let Some(error) = verification_error {
        return Err(fail_job(
            &db,
            &mut job,
            "POST_INSTALL_VERIFICATION_FAILED",
            error,
        ));
    }
    job.state = "succeeded".to_string();
    job.completed_at = Some(now());
    save_job(&db, &job)?;
    Ok(after)
}

#[tauri::command]
pub async fn cancel_desktop_lifecycle_job(
    operations: State<'_, DesktopLifecycleOperationState>,
    #[allow(non_snake_case)] jobId: String,
) -> Result<bool, String> {
    Ok(operations.cancel_job(&jobId).await)
}

#[tauri::command]
pub async fn get_desktop_lifecycle_job(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] jobId: String,
) -> Result<Option<DesktopLifecycleJob>, String> {
    state
        .db
        .get_lifecycle_job(&jobId)
        .map_err(|error| error.to_string())?
        .map(job_from_record)
        .transpose()
}

#[tauri::command]
pub async fn list_desktop_lifecycle_jobs(
    state: State<'_, AppState>,
    app: Option<String>,
) -> Result<Vec<DesktopLifecycleJob>, String> {
    state
        .db
        .list_lifecycle_jobs(app.as_deref(), 20)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(job_from_record)
        .collect()
}

#[tauri::command]
pub async fn launch_desktop_app(app: String) -> Result<(), String> {
    let manifest = manifest(&app)?;
    tokio::task::spawn_blocking(move || {
        let status = detect_desktop_app(manifest)?;
        if !status.installed {
            return Err(format!("{} is not installed", manifest.display_name));
        }

        #[cfg(target_os = "windows")]
        {
            if status.package_identity.is_some() {
                let family_name = {
                    let script = format!(
                        "(Get-AppxPackage -Name '{}').PackageFamilyName",
                        manifest.windows_package_name
                    );
                    powershell_output(&script, "desktop package family lookup").ok()
                }
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "Desktop package family name is unavailable".to_string())?;
                let app_user_model_id =
                    format!("{}!{}", family_name.trim(), manifest.windows_app_id_suffix);
                let mut command = Command::new("explorer.exe");
                command.arg(format!("shell:AppsFolder\\{app_user_model_id}"));
                command.spawn().map_err(|error| {
                    format!("Failed to launch {}: {error}", manifest.display_name)
                })?;
            } else {
                let target = status
                    .launch_target
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| "Desktop executable is unavailable".to_string())?;
                Command::new(target).spawn().map_err(|error| {
                    format!("Failed to launch {}: {error}", manifest.display_name)
                })?;
            }
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        {
            let status = Command::new("open")
                .args(["-a", manifest.macos_app_name])
                .status()
                .map_err(|error| format!("Failed to launch {}: {error}", manifest.display_name))?;
            if !status.success() {
                return Err(format!("Failed to launch {}", manifest.display_name));
            }
            return Ok(());
        }

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        Err(format!(
            "{} is not supported on the current platform",
            manifest.display_name
        ))
    })
    .await
    .map_err(|error| format!("Desktop launch task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_registry_rejects_unknown_apps() {
        assert!(manifest("deepseek-desktop").is_err());
    }

    #[test]
    fn dotted_versions_compare_numerically() {
        assert!(version_is_newer("1.10.0", "1.9.9"));
        assert!(version_is_newer("26.901.6512.0", "26.901.6511.0"));
        assert!(!version_is_newer("1.2.0", "1.2"));
        assert!(!version_is_newer("1.1", "1.2"));
    }
}
