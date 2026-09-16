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
use std::process::{Command, Stdio};
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
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    macos_bundle_path: &'static str,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    macos_app_name: &'static str,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    macos_cask: &'static str,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default)]
    pub installations: Vec<DesktopInstallation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopInstallation {
    pub version: String,
    pub path: String,
    pub launch_target: Option<String>,
    pub package_identity: Option<String>,
    pub installation_source: String,
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

#[cfg(target_os = "windows")]
#[derive(Debug, Deserialize)]
struct AppxRecords {
    records: Vec<AppxRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// 按应用串行化生命周期操作、跟踪可取消任务的共享状态。
/// 机制与具体应用无关，CLI（cli_lifecycle）与桌面应用共用同一实例。
#[derive(Default)]
pub struct DesktopLifecycleOperationState {
    locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
    cancellations: RwLock<HashMap<String, Arc<AtomicBool>>>,
}

impl DesktopLifecycleOperationState {
    pub(crate) async fn lock(&self, app_id: &str) -> OwnedMutexGuard<()> {
        let existing = { self.locks.read().await.get(app_id).cloned() };
        let lock = if let Some(lock) = existing {
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

    pub(crate) async fn register_job(&self, job_id: &str) -> Arc<AtomicBool> {
        let cancellation = Arc::new(AtomicBool::new(false));
        self.cancellations
            .write()
            .await
            .insert(job_id.to_string(), cancellation.clone());
        cancellation
    }

    pub(crate) async fn finish_job(&self, job_id: &str) {
        self.cancellations.write().await.remove(job_id);
    }

    pub(crate) async fn cancel_job(&self, job_id: &str) -> bool {
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
    #[serde(default)]
    pub logs: Vec<DesktopLifecycleLogEntry>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopLifecycleLogEntry {
    pub at: i64,
    pub level: String,
    pub step: String,
    pub message: String,
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
            "logs": job.logs,
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
    let logs = serde_json::from_str::<serde_json::Value>(&record.plan_json)
        .ok()
        .and_then(|value| value.get("logs").cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
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
        logs,
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
    // Drain both pipes while the installer is running. Waiting first and only
    // reading afterwards can deadlock when winget/Homebrew fills an OS pipe.
    let stdout_reader = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut output = String::new();
            let _ = pipe.read_to_string(&mut output);
            output
        })
    });
    let stderr_reader = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut output = String::new();
            let _ = pipe.read_to_string(&mut output);
            output
        })
    });
    let status = loop {
        if cancellation.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(reader) = stdout_reader {
                let _ = reader.join();
            }
            if let Some(reader) = stderr_reader {
                let _ = reader.join();
            }
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
    let stdout = stdout_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let stderr = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
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
    let winget_available = Command::new("winget.exe")
        .arg("--version")
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    let script = format!(
        r#"[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$records = @()
$packages = Get-AppxPackage -Name '{}' | Sort-Object Version -Descending
foreach ($pkg in $packages) {{
  $records += [pscustomobject]@{{
    kind = 'appx'
    version = $pkg.Version.ToString()
    package_full_name = $pkg.PackageFullName
    package_family_name = $pkg.PackageFamilyName
    install_location = $pkg.InstallLocation
    launch_target = $null
  }}
}}
$uninstallRoots = @(
  'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
)
$entries = Get-ItemProperty $uninstallRoots -ErrorAction SilentlyContinue |
  Where-Object {{ $_.DisplayName -eq '{}' }} | Sort-Object DisplayVersion -Descending
foreach ($entry in $entries) {{
  $launch = $entry.DisplayIcon
  if ($launch) {{ $launch = $launch.Trim('"').Split(',')[0] }}
  $records += [pscustomobject]@{{
    kind = 'win32'
    version = [string]$entry.DisplayVersion
    package_full_name = $null
    package_family_name = $null
    install_location = [string]$entry.InstallLocation
    launch_target = $launch
  }}
}}
[pscustomobject]@{{ records = @($records) }} | ConvertTo-Json -Depth 4 -Compress"#,
        manifest.windows_package_name, manifest.windows_display_name
    );
    let output = powershell_output(&script, "desktop application detection")?;
    let mut records = serde_json::from_str::<AppxRecords>(&output)
        .map_err(|error| format!("Invalid desktop package metadata: {error}"))?
        .records;
    if records.is_empty() {
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
            can_install: winget_available,
            can_update: false,
            can_uninstall: false,
            can_launch: false,
            reason: (!winget_available).then(|| "未找到 Winget，无法执行自动安装".to_string()),
            installations: Vec::new(),
        });
    }

    for record in &records {
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
    }
    records.sort_by(|left, right| {
        numeric_version_parts(&right.version).cmp(&numeric_version_parts(&left.version))
    });
    let installations = records
        .iter()
        .map(|record| DesktopInstallation {
            version: record.version.clone(),
            path: record.install_location.clone(),
            launch_target: record.launch_target.clone(),
            package_identity: record.package_full_name.clone(),
            installation_source: if record.kind == "win32" {
                "official_exe"
            } else if manifest.id == "codex-desktop" {
                "microsoft_store"
            } else {
                "official_appx"
            }
            .to_string(),
        })
        .collect::<Vec<_>>();
    let primary = installations
        .first()
        .expect("non-empty installations after detection");
    let primary_is_appx = primary.package_identity.is_some();

    Ok(DesktopAppStatus {
        id: manifest.id.to_string(),
        display_name: manifest.display_name.to_string(),
        installed: true,
        version: Some(primary.version.clone()),
        latest_version: None,
        path: Some(primary.path.clone()),
        launch_target: primary.launch_target.clone(),
        package_identity: primary.package_identity.clone(),
        installation_source: primary.installation_source.clone(),
        can_install: false,
        can_update: winget_available,
        can_uninstall: primary_is_appx || winget_available,
        can_launch: true,
        reason: (!winget_available).then(|| {
            if primary_is_appx {
                "未找到 Winget，仍可启动和卸载，但不能自动更新"
            } else {
                "未找到 Winget，仍可启动，但不能自动更新或卸载"
            }
            .to_string()
        }),
        installations,
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
        version: version.clone(),
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
        installations: if installed {
            vec![DesktopInstallation {
                version: version.unwrap_or_default(),
                path: manifest.macos_bundle_path.to_string(),
                launch_target: Some(manifest.macos_bundle_path.to_string()),
                package_identity: None,
                installation_source: "application_bundle".to_string(),
            }]
        } else {
            Vec::new()
        },
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
        installations: Vec::new(),
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

fn same_installation(left: &DesktopInstallation, right: &DesktopInstallation) -> bool {
    match (
        left.package_identity.as_deref(),
        right.package_identity.as_deref(),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => {
            left.installation_source == right.installation_source
                && !left.path.is_empty()
                && left.path.eq_ignore_ascii_case(&right.path)
        }
    }
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
        DesktopLifecycleAction::Uninstall => before
            .installations
            .first()
            .map(|target| {
                !after
                    .installations
                    .iter()
                    .any(|candidate| same_installation(target, candidate))
            })
            .unwrap_or(!after.installed),
    }
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

trait DesktopLifecycleRuntime: Send + Sync {
    fn probe(&self, manifest: DesktopAppManifest) -> Result<DesktopAppStatus, String>;

    fn latest_version(&self, manifest: DesktopAppManifest) -> Result<String, String>;

    fn verification_retries(&self) -> usize {
        29
    }

    fn verification_delay(&self) -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }

    fn execute(
        &self,
        manifest: DesktopAppManifest,
        action: DesktopLifecycleAction,
        before: &DesktopAppStatus,
        cancellation: &AtomicBool,
    ) -> Result<(), String>;
}

struct SystemDesktopLifecycleRuntime;

impl DesktopLifecycleRuntime for SystemDesktopLifecycleRuntime {
    fn probe(&self, manifest: DesktopAppManifest) -> Result<DesktopAppStatus, String> {
        detect_desktop_app(manifest)
    }

    fn latest_version(&self, manifest: DesktopAppManifest) -> Result<String, String> {
        latest_winget_version(manifest)
    }

    fn execute(
        &self,
        manifest: DesktopAppManifest,
        action: DesktopLifecycleAction,
        before: &DesktopAppStatus,
        cancellation: &AtomicBool,
    ) -> Result<(), String> {
        match action {
            DesktopLifecycleAction::Install | DesktopLifecycleAction::Update => {
                #[cfg(target_os = "windows")]
                {
                    run_winget_action(manifest, action, cancellation)
                }
                #[cfg(target_os = "macos")]
                {
                    run_macos_action(manifest, action, cancellation)
                }
                #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                {
                    Err(format!(
                        "Automatic desktop {} is not supported on this platform",
                        action.as_str()
                    ))
                }
            }
            DesktopLifecycleAction::Uninstall => {
                #[cfg(target_os = "windows")]
                {
                    if let Some(identity) = before.package_identity.as_deref() {
                        uninstall_appx(manifest, identity, cancellation)
                    } else {
                        run_winget_uninstall(manifest, cancellation)
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    run_macos_action(manifest, action, cancellation)
                }
                #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                {
                    Err("Automatic desktop uninstall is not supported on this platform".to_string())
                }
            }
        }
    }
}

fn detect_after_action_with_runtime(
    runtime: &dyn DesktopLifecycleRuntime,
    manifest: DesktopAppManifest,
    action: DesktopLifecycleAction,
    before: &DesktopAppStatus,
    cancellation: &AtomicBool,
) -> Result<DesktopAppStatus, String> {
    if cancellation.load(Ordering::SeqCst) {
        return Err("JOB_CANCELLED".to_string());
    }
    let mut last = runtime.probe(manifest)?;
    if verification_satisfied(action, before, &last) {
        return Ok(last);
    }
    // Store/Appx registration can trail the installer process. Poll briefly
    // before declaring verification failure, while keeping success tied to a
    // real post-action probe.
    for _ in 0..runtime.verification_retries() {
        if cancellation.load(Ordering::SeqCst) {
            return Err("JOB_CANCELLED".to_string());
        }
        std::thread::sleep(runtime.verification_delay());
        last = runtime.probe(manifest)?;
        if verification_satisfied(action, before, &last) {
            return Ok(last);
        }
    }
    Ok(last)
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
    run_desktop_app_lifecycle_action_with_runtime(
        state.db.clone(),
        &operations,
        app,
        action,
        jobId,
        Arc::new(SystemDesktopLifecycleRuntime),
    )
    .await
}

async fn run_desktop_app_lifecycle_action_with_runtime(
    db: Arc<crate::database::Database>,
    operations: &DesktopLifecycleOperationState,
    app: String,
    action: String,
    job_id: Option<String>,
    runtime: Arc<dyn DesktopLifecycleRuntime>,
) -> Result<DesktopAppStatus, String> {
    let manifest = manifest(&app)?;
    let action = DesktopLifecycleAction::parse(&action)?;
    let _guard = operations.lock(&app).await;
    let job_id = job_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    if let Some(existing) = db
        .get_lifecycle_job(&job_id)
        .map_err(|error| error.to_string())?
    {
        let existing = job_from_record(existing)?;
        if existing.state == "succeeded" {
            if let Some(status) = existing.post_probe {
                return Ok(status);
            }
        }
        return Err(existing
            .error_message
            .unwrap_or_else(|| "相同请求仍在处理中，请稍后重试".to_string()));
    }
    let cancellation = operations.register_job(&job_id).await;
    let result = tokio::task::spawn_blocking({
        let job_id = job_id.clone();
        move || run_lifecycle_job(db, job_id, manifest, action, cancellation, runtime)
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
    job.logs.push(DesktopLifecycleLogEntry {
        at: now(),
        level: "error".to_string(),
        step: job.state.clone(),
        message: message.clone(),
    });
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
    runtime: Arc<dyn DesktopLifecycleRuntime>,
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
        logs: vec![DesktopLifecycleLogEntry {
            at: created_at,
            level: "info".to_string(),
            step: "queued".to_string(),
            message: format!("已创建 {} 任务", action.as_str()),
        }],
        created_at,
        started_at: None,
        completed_at: None,
    };
    save_job(&db, &job)?;
    job.state = "running".to_string();
    job.started_at = Some(now());
    job.logs.push(DesktopLifecycleLogEntry {
        at: now(),
        level: "info".to_string(),
        step: "detecting".to_string(),
        message: "正在检测当前安装状态".to_string(),
    });
    save_job(&db, &job)?;

    let before = match runtime.probe(manifest) {
        Ok(status) => status,
        Err(error) => return Err(fail_job(&db, &mut job, "APP_PROBE_FAILED", error)),
    };
    job.pre_probe = Some(before.clone());
    job.logs.push(DesktopLifecycleLogEntry {
        at: now(),
        level: "info".to_string(),
        step: "detected".to_string(),
        message: if before.installed {
            format!(
                "已检测到 {}{}",
                manifest.display_name,
                before
                    .version
                    .as_deref()
                    .map(|version| format!(" {version}"))
                    .unwrap_or_default()
            )
        } else {
            format!("未检测到 {}", manifest.display_name)
        },
    });
    if let Err(error) = save_job(&db, &job) {
        return Err(fail_job(&db, &mut job, "JOB_PERSIST_FAILED", error));
    }

    job.logs.push(DesktopLifecycleLogEntry {
        at: now(),
        level: "info".to_string(),
        step: "executing".to_string(),
        message: format!("正在通过已登记的官方安装源执行 {}", action.as_str()),
    });
    save_job(&db, &job)?;
    let execute_result = match action {
        DesktopLifecycleAction::Install if before.installed => {
            Err(format!("{} is already installed", manifest.display_name))
        }
        DesktopLifecycleAction::Update if !before.installed => {
            Err(format!("{} is not installed", manifest.display_name))
        }
        DesktopLifecycleAction::Update => match runtime.latest_version(manifest) {
            Ok(latest) => {
                let current = before.version.as_deref().unwrap_or_default();
                if !version_is_newer(&latest, current) {
                    let mut current_status = before.clone();
                    current_status.latest_version = Some(latest);
                    job.post_probe = Some(current_status.clone());
                    job.state = "succeeded".to_string();
                    job.logs.push(DesktopLifecycleLogEntry {
                        at: now(),
                        level: "info".to_string(),
                        step: "completed".to_string(),
                        message: "当前已是最新版本，无需更新".to_string(),
                    });
                    job.completed_at = Some(now());
                    save_job(&db, &job)?;
                    return Ok(current_status);
                }
                runtime.execute(manifest, action, &before, &cancellation)
            }
            Err(error) => Err(error),
        },
        DesktopLifecycleAction::Uninstall if !before.installed => {
            job.post_probe = Some(before.clone());
            job.state = "succeeded".to_string();
            job.logs.push(DesktopLifecycleLogEntry {
                at: now(),
                level: "info".to_string(),
                step: "completed".to_string(),
                message: "应用已经处于未安装状态，无需卸载".to_string(),
            });
            job.completed_at = Some(now());
            save_job(&db, &job)?;
            return Ok(before);
        }
        _ => runtime.execute(manifest, action, &before, &cancellation),
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
            job.logs.push(DesktopLifecycleLogEntry {
                at: now(),
                level: "warning".to_string(),
                step: "cancelled".to_string(),
                message: "用户已停止操作".to_string(),
            });
            job.completed_at = Some(now());
            save_job(&db, &job)?;
            return Err("操作已停止".to_string());
        }
        return Err(fail_job(&db, &mut job, code, error));
    }

    job.state = "verifying".to_string();
    job.logs.push(DesktopLifecycleLogEntry {
        at: now(),
        level: "info".to_string(),
        step: "verifying".to_string(),
        message: "安装器已结束，正在重新检测版本和安装状态".to_string(),
    });
    save_job(&db, &job)?;
    let mut after = match detect_after_action_with_runtime(
        runtime.as_ref(),
        manifest,
        action,
        &before,
        &cancellation,
    ) {
        Ok(status) => status,
        Err(error) if error == "JOB_CANCELLED" => {
            job.state = "cancelled".to_string();
            job.error_code = Some("JOB_CANCELLED".to_string());
            job.error_message = Some("操作已停止".to_string());
            job.logs.push(DesktopLifecycleLogEntry {
                at: now(),
                level: "warning".to_string(),
                step: "cancelled".to_string(),
                message: "用户已停止结果检测".to_string(),
            });
            job.completed_at = Some(now());
            save_job(&db, &job)?;
            return Err("操作已停止".to_string());
        }
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
        DesktopLifecycleAction::Uninstall if !verification_satisfied(action, &before, &after) => {
            Some(format!(
                "{} uninstall completed but the package is still installed",
                manifest.display_name
            ))
        }
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
    if matches!(action, DesktopLifecycleAction::Uninstall) && after.installed {
        after.reason = Some(format!(
            "已移除所选安装，但仍检测到另一份 {}（{}）",
            manifest.display_name,
            after.version.as_deref().unwrap_or("版本未知")
        ));
        job.post_probe = Some(after.clone());
        job.logs.push(DesktopLifecycleLogEntry {
            at: now(),
            level: "warning".to_string(),
            step: "completed".to_string(),
            message: after.reason.clone().unwrap_or_default(),
        });
    }
    job.state = "succeeded".to_string();
    job.logs.push(DesktopLifecycleLogEntry {
        at: now(),
        level: "info".to_string(),
        step: "completed".to_string(),
        message: match action {
            DesktopLifecycleAction::Install => "安装完成并已通过检测",
            DesktopLifecycleAction::Update => "更新完成并已检测到版本变化",
            DesktopLifecycleAction::Uninstall => "卸载完成并确认应用已移除",
        }
        .to_string(),
    });
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
#[allow(clippy::needless_return)] // cfg-gated blocks end in `return` on each platform
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
    use std::sync::Mutex;

    struct FakeDesktopLifecycleRuntime {
        probes: Mutex<Vec<Result<DesktopAppStatus, String>>>,
        latest_version: Result<String, String>,
        execution_result: Result<(), String>,
        executions: Mutex<Vec<DesktopLifecycleAction>>,
    }

    impl FakeDesktopLifecycleRuntime {
        fn new(
            probes: Vec<Result<DesktopAppStatus, String>>,
            latest_version: Result<&str, &str>,
            execution_result: Result<(), &str>,
        ) -> Self {
            Self {
                probes: Mutex::new(probes),
                latest_version: latest_version.map(str::to_string).map_err(str::to_string),
                execution_result: execution_result.map_err(str::to_string),
                executions: Mutex::new(Vec::new()),
            }
        }

        fn executions(&self) -> Vec<DesktopLifecycleAction> {
            self.executions.lock().expect("execution lock").clone()
        }
    }

    impl DesktopLifecycleRuntime for FakeDesktopLifecycleRuntime {
        fn probe(&self, _manifest: DesktopAppManifest) -> Result<DesktopAppStatus, String> {
            self.probes.lock().expect("probe lock").remove(0)
        }

        fn latest_version(&self, _manifest: DesktopAppManifest) -> Result<String, String> {
            self.latest_version.clone()
        }

        fn verification_retries(&self) -> usize {
            0
        }

        fn verification_delay(&self) -> std::time::Duration {
            std::time::Duration::ZERO
        }

        fn execute(
            &self,
            _manifest: DesktopAppManifest,
            action: DesktopLifecycleAction,
            _before: &DesktopAppStatus,
            cancellation: &AtomicBool,
        ) -> Result<(), String> {
            self.executions.lock().expect("execution lock").push(action);
            if cancellation.load(Ordering::SeqCst) {
                return Err("JOB_CANCELLED".to_string());
            }
            self.execution_result.clone()
        }
    }

    async fn run_fake_lifecycle(
        db: Arc<crate::database::Database>,
        runtime: Arc<FakeDesktopLifecycleRuntime>,
        app: &str,
        action: &str,
        job_id: &str,
    ) -> Result<DesktopAppStatus, String> {
        run_desktop_app_lifecycle_action_with_runtime(
            db,
            &DesktopLifecycleOperationState::default(),
            app.to_string(),
            action.to_string(),
            Some(job_id.to_string()),
            runtime,
        )
        .await
    }

    fn saved_job(db: &crate::database::Database, job_id: &str) -> DesktopLifecycleJob {
        db.get_lifecycle_job(job_id)
            .expect("load job")
            .map(job_from_record)
            .expect("job exists")
            .expect("parse job")
    }

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

    fn status(installed: bool, version: Option<&str>) -> DesktopAppStatus {
        DesktopAppStatus {
            id: "codex-desktop".to_string(),
            display_name: "Codex Desktop".to_string(),
            installed,
            version: version.map(str::to_string),
            latest_version: None,
            path: None,
            launch_target: None,
            package_identity: None,
            installation_source: if installed {
                "microsoft_store"
            } else {
                "not_installed"
            }
            .to_string(),
            can_install: !installed,
            can_update: installed,
            can_uninstall: installed,
            can_launch: installed,
            reason: None,
            installations: Vec::new(),
        }
    }

    #[test]
    fn lifecycle_success_requires_a_real_post_action_state_change() {
        let missing = status(false, None);
        let v1 = status(true, Some("1.0.0"));
        let v2 = status(true, Some("1.1.0"));

        assert!(verification_satisfied(
            DesktopLifecycleAction::Install,
            &missing,
            &v1
        ));
        assert!(!verification_satisfied(
            DesktopLifecycleAction::Install,
            &missing,
            &missing
        ));
        assert!(verification_satisfied(
            DesktopLifecycleAction::Update,
            &v1,
            &v2
        ));
        assert!(!verification_satisfied(
            DesktopLifecycleAction::Update,
            &v1,
            &v1
        ));
        assert!(verification_satisfied(
            DesktopLifecycleAction::Uninstall,
            &v1,
            &missing
        ));
        assert!(!verification_satisfied(
            DesktopLifecycleAction::Uninstall,
            &v1,
            &v1
        ));

        let mut appx = v1.clone();
        appx.installations = vec![DesktopInstallation {
            version: "1.0.0".to_string(),
            path: "C:\\Program Files\\WindowsApps\\Claude".to_string(),
            launch_target: None,
            package_identity: Some("Claude_1.0.0_x64".to_string()),
            installation_source: "official_appx".to_string(),
        }];
        let mut remaining_exe = v2;
        remaining_exe.installations = vec![DesktopInstallation {
            version: "1.1.0".to_string(),
            path: "C:\\Users\\test\\AppData\\Local\\Claude".to_string(),
            launch_target: Some("claude.exe".to_string()),
            package_identity: None,
            installation_source: "official_exe".to_string(),
        }];
        assert!(verification_satisfied(
            DesktopLifecycleAction::Uninstall,
            &appx,
            &remaining_exe
        ));
    }

    #[test]
    fn lifecycle_job_round_trip_preserves_structured_logs() {
        let job = DesktopLifecycleJob {
            id: "job-log".to_string(),
            app_id: "codex-desktop".to_string(),
            component: "desktop".to_string(),
            action: "install".to_string(),
            state: "verifying".to_string(),
            pre_probe: Some(status(false, None)),
            post_probe: None,
            error_code: None,
            error_message: None,
            logs: vec![DesktopLifecycleLogEntry {
                at: 11,
                level: "info".to_string(),
                step: "verifying".to_string(),
                message: "正在重新检测安装结果".to_string(),
            }],
            created_at: 10,
            started_at: Some(11),
            completed_at: None,
        };

        let restored =
            job_from_record(job_record(&job).expect("serialize job")).expect("deserialize job");
        assert_eq!(restored.logs.len(), 1);
        assert_eq!(restored.logs[0].step, "verifying");
        assert_eq!(restored.logs[0].message, "正在重新检测安装结果");
    }

    #[tokio::test]
    async fn fake_runtime_persists_install_success_and_replays_only_verified_success() {
        let db = Arc::new(crate::database::Database::memory().expect("memory database"));
        let runtime = Arc::new(FakeDesktopLifecycleRuntime::new(
            vec![Ok(status(false, None)), Ok(status(true, Some("1.0.0")))],
            Ok("1.0.0"),
            Ok(()),
        ));

        let completed = run_fake_lifecycle(
            db.clone(),
            runtime.clone(),
            "codex-desktop",
            "install",
            "desktop-install",
        )
        .await
        .expect("install succeeds");
        assert!(completed.installed);
        assert_eq!(runtime.executions(), vec![DesktopLifecycleAction::Install]);

        let job = saved_job(&db, "desktop-install");
        assert_eq!(job.state, "succeeded");
        assert_eq!(job.pre_probe, Some(status(false, None)));
        assert_eq!(job.post_probe, Some(status(true, Some("1.0.0"))));
        assert!(job.completed_at.is_some());

        let replay = run_fake_lifecycle(
            db,
            runtime.clone(),
            "codex-desktop",
            "install",
            "desktop-install",
        )
        .await
        .expect("successful verified job replays");
        assert_eq!(replay.version.as_deref(), Some("1.0.0"));
        assert_eq!(runtime.executions(), vec![DesktopLifecycleAction::Install]);
    }

    #[tokio::test]
    async fn fake_runtime_persists_update_success_and_noop_without_execution() {
        let db = Arc::new(crate::database::Database::memory().expect("memory database"));
        let update_runtime = Arc::new(FakeDesktopLifecycleRuntime::new(
            vec![
                Ok(status(true, Some("1.0.0"))),
                Ok(status(true, Some("1.1.0"))),
            ],
            Ok("1.1.0"),
            Ok(()),
        ));
        run_fake_lifecycle(
            db.clone(),
            update_runtime.clone(),
            "codex-desktop",
            "update",
            "desktop-update",
        )
        .await
        .expect("update succeeds");
        assert_eq!(
            update_runtime.executions(),
            vec![DesktopLifecycleAction::Update]
        );
        assert_eq!(
            saved_job(&db, "desktop-update")
                .post_probe
                .and_then(|probe| probe.version),
            Some("1.1.0".to_string())
        );

        let noop_runtime = Arc::new(FakeDesktopLifecycleRuntime::new(
            vec![Ok(status(true, Some("1.1.0")))],
            Ok("1.1.0"),
            Ok(()),
        ));
        let completed = run_fake_lifecycle(
            db.clone(),
            noop_runtime.clone(),
            "codex-desktop",
            "update",
            "desktop-update-noop",
        )
        .await
        .expect("current update is a no-op");
        assert_eq!(completed.latest_version.as_deref(), Some("1.1.0"));
        assert!(noop_runtime.executions().is_empty());
        assert_eq!(saved_job(&db, "desktop-update-noop").state, "succeeded");
    }

    #[tokio::test]
    async fn fake_runtime_persists_uninstall_noop_and_verification_failure() {
        let db = Arc::new(crate::database::Database::memory().expect("memory database"));
        let noop_runtime = Arc::new(FakeDesktopLifecycleRuntime::new(
            vec![Ok(status(false, None))],
            Ok("1.0.0"),
            Ok(()),
        ));
        run_fake_lifecycle(
            db.clone(),
            noop_runtime.clone(),
            "codex-desktop",
            "uninstall",
            "desktop-uninstall-noop",
        )
        .await
        .expect("absent uninstall is a no-op");
        assert!(noop_runtime.executions().is_empty());
        assert_eq!(saved_job(&db, "desktop-uninstall-noop").state, "succeeded");

        let verification_runtime = Arc::new(FakeDesktopLifecycleRuntime::new(
            vec![Ok(status(false, None)), Ok(status(false, None))],
            Ok("1.0.0"),
            Ok(()),
        ));
        let error = run_fake_lifecycle(
            db.clone(),
            verification_runtime.clone(),
            "codex-desktop",
            "install",
            "desktop-verification-failure",
        )
        .await
        .expect_err("unchanged post-probe fails verification");
        assert!(error.contains("not detected"));
        let job = saved_job(&db, "desktop-verification-failure");
        assert_eq!(job.state, "failed");
        assert_eq!(
            job.error_code.as_deref(),
            Some("POST_INSTALL_VERIFICATION_FAILED")
        );
        assert_eq!(job.post_probe, Some(status(false, None)));

        let duplicate_error = run_fake_lifecycle(
            db,
            verification_runtime,
            "codex-desktop",
            "install",
            "desktop-verification-failure",
        )
        .await
        .expect_err("failed job id does not replay post-probe");
        assert_eq!(duplicate_error, error);
    }

    #[tokio::test]
    async fn fake_runtime_persists_execution_failure_and_cancellation() {
        let db = Arc::new(crate::database::Database::memory().expect("memory database"));
        let failed_runtime = Arc::new(FakeDesktopLifecycleRuntime::new(
            vec![Ok(status(true, Some("1.0.0")))],
            Ok("1.1.0"),
            Err("installer failed"),
        ));
        let error = run_fake_lifecycle(
            db.clone(),
            failed_runtime,
            "codex-desktop",
            "update",
            "desktop-execution-failure",
        )
        .await
        .expect_err("execution fails");
        assert_eq!(error, "installer failed");
        let job = saved_job(&db, "desktop-execution-failure");
        assert_eq!(job.state, "failed");
        assert_eq!(
            job.error_code.as_deref(),
            Some("LIFECYCLE_EXECUTION_FAILED")
        );
        assert!(job.post_probe.is_none());

        let cancelled_runtime = Arc::new(FakeDesktopLifecycleRuntime::new(
            vec![Ok(status(true, Some("1.0.0")))],
            Ok("1.1.0"),
            Err("JOB_CANCELLED"),
        ));
        let error = run_fake_lifecycle(
            db.clone(),
            cancelled_runtime,
            "codex-desktop",
            "update",
            "desktop-cancelled",
        )
        .await
        .expect_err("execution cancellation fails request");
        assert_eq!(error, "操作已停止");
        let job = saved_job(&db, "desktop-cancelled");
        assert_eq!(job.state, "cancelled");
        assert_eq!(job.error_code.as_deref(), Some("JOB_CANCELLED"));
        assert!(job.completed_at.is_some());
    }
}
