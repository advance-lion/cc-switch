//! Lifecycle management for registered desktop applications.
//!
//! Desktop packages are deliberately separate from CLI tools. Every operation
//! resolves through this fixed registry, so the renderer can never provide an
//! executable path, package identifier, download URL, or arbitrary command.

use serde::{Deserialize, Serialize};
use std::process::{Child, Command, Stdio};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Clone, Copy)]
struct DesktopAppManifest {
    id: &'static str,
    display_name: &'static str,
    windows_package_name: &'static str,
    windows_app_id_suffix: &'static str,
    winget_id: &'static str,
    winget_source: &'static str,
    macos_bundle_path: &'static str,
    macos_app_name: &'static str,
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
    windows_app_id_suffix: "App",
    // Official Microsoft Store product id from the OpenAI Codex Windows page.
    winget_id: "9PLM9XGG6VKS",
    winget_source: "msstore",
    macos_bundle_path: "/Applications/Codex.app",
    macos_app_name: "Codex",
};

const CLAUDE_DESKTOP: DesktopAppManifest = DesktopAppManifest {
    id: "claude-desktop",
    display_name: "Claude Desktop",
    windows_package_name: "Claude",
    windows_app_id_suffix: "Claude",
    winget_id: "Anthropic.Claude",
    winget_source: "winget",
    macos_bundle_path: "/Applications/Claude.app",
    macos_app_name: "Claude",
};

fn manifest(app: &str) -> Result<DesktopAppManifest, String> {
    match app {
        "codex-desktop" => Ok(CODEX_DESKTOP),
        "claude-desktop" => Ok(CLAUDE_DESKTOP),
        _ => Err(format!("Unsupported desktop application: {app}")),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DesktopAppStatus {
    pub id: String,
    pub display_name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub latest_version: Option<String>,
    pub path: Option<String>,
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
    version: String,
    package_full_name: String,
    package_family_name: String,
    install_location: String,
}

#[derive(Clone, Copy)]
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
}

#[cfg(target_os = "windows")]
fn command_output(command: &mut Command, label: &str) -> Result<String, String> {
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
if ($null -eq $pkg) {{ Write-Output 'null'; exit 0 }}
[pscustomobject]@{{
  version = $pkg.Version.ToString()
  package_full_name = $pkg.PackageFullName
  package_family_name = $pkg.PackageFamilyName
  install_location = $pkg.InstallLocation
}} | ConvertTo-Json -Compress"#,
        manifest.windows_package_name
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
    if !record.package_full_name.starts_with(manifest.windows_package_name)
        || !record
            .package_family_name
            .starts_with(manifest.windows_package_name)
    {
        return Err(format!(
            "Detected package identity does not match {}",
            manifest.display_name
        ));
    }

    Ok(DesktopAppStatus {
        id: manifest.id.to_string(),
        display_name: manifest.display_name.to_string(),
        installed: true,
        version: Some(record.version),
        latest_version: None,
        path: Some(record.install_location),
        package_identity: Some(record.package_full_name),
        installation_source: if manifest.id == "codex-desktop" {
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
    Ok(DesktopAppStatus {
        id: manifest.id.to_string(),
        display_name: manifest.display_name.to_string(),
        installed,
        version,
        latest_version: None,
        path: installed.then(|| manifest.macos_bundle_path.to_string()),
        package_identity: None,
        installation_source: if installed { "application_bundle" } else { "not_installed" }
            .to_string(),
        can_install: false,
        can_update: false,
        can_uninstall: false,
        can_launch: installed,
        reason: Some("Automatic desktop installation is currently supported on Windows only.".to_string()),
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
        package_identity: None,
        installation_source: "unsupported_platform".to_string(),
        can_install: false,
        can_update: false,
        can_uninstall: false,
        can_launch: false,
        reason: Some("This desktop application is not supported on the current platform.".to_string()),
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

#[cfg(not(target_os = "windows"))]
fn latest_winget_version(manifest: DesktopAppManifest) -> Result<String, String> {
    Err(format!(
        "Automatic {} update checks are currently supported on Windows only",
        manifest.display_name
    ))
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
) -> Result<(), String> {
    let mut command = winget_action_command(manifest, action)?;
    command_output(&mut command, "desktop lifecycle action").map(|_| ())
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
    #[cfg(not(target_os = "windows"))]
    return Err(format!(
        "{} 的 AI 辅助桌面安装当前仅支持 Windows",
        manifest.display_name
    ));
    #[cfg(target_os = "windows")]
    Ok(RegisteredDesktopAssistantInstall {
        app_id: manifest.id.to_string(),
        display_name: manifest.display_name.to_string(),
        version: "latest".to_string(),
        official_source: if manifest.id == "codex-desktop" {
            "https://apps.microsoft.com/detail/9PLM9XGG6VKS".to_string()
        } else {
            "https://claude.ai/download".to_string()
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
    #[cfg(not(target_os = "windows"))]
    Err(format!(
        "{} 的 AI 辅助桌面安装当前仅支持 Windows",
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
fn uninstall_appx(manifest: DesktopAppManifest, package_identity: &str) -> Result<(), String> {
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
    powershell_output(&script, "desktop application uninstall").map(|_| ())
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
    app: String,
    action: String,
) -> Result<DesktopAppStatus, String> {
    let manifest = manifest(&app)?;
    let action = DesktopLifecycleAction::parse(&action)?;
    tokio::task::spawn_blocking(move || {
        let before = detect_desktop_app(manifest)?;
        match action {
            DesktopLifecycleAction::Install => {
                if before.installed {
                    return Err(format!("{} is already installed", manifest.display_name));
                }
                #[cfg(target_os = "windows")]
                run_winget_action(manifest, action)?;
                #[cfg(not(target_os = "windows"))]
                return Err("Automatic desktop installation is supported on Windows only".to_string());
            }
            DesktopLifecycleAction::Update => {
                if !before.installed {
                    return Err(format!("{} is not installed", manifest.display_name));
                }
                let latest = latest_winget_version(manifest)?;
                let current = before.version.as_deref().unwrap_or_default();
                if !version_is_newer(&latest, current) {
                    let mut current_status = before;
                    current_status.latest_version = Some(latest);
                    return Ok(current_status);
                }
                #[cfg(target_os = "windows")]
                run_winget_action(manifest, action)?;
                #[cfg(not(target_os = "windows"))]
                return Err("Automatic desktop updates are supported on Windows only".to_string());
            }
            DesktopLifecycleAction::Uninstall => {
                if !before.installed {
                    return Ok(before);
                }
                #[cfg(target_os = "windows")]
                uninstall_appx(
                    manifest,
                    before
                        .package_identity
                        .as_deref()
                        .ok_or_else(|| "Desktop package identity is unavailable".to_string())?,
                )?;
                #[cfg(not(target_os = "windows"))]
                return Err("Automatic desktop uninstall is supported on Windows only".to_string());
            }
        }

        let after = detect_desktop_app(manifest)?;
        match action {
            DesktopLifecycleAction::Install if !after.installed => Err(format!(
                "{} installer completed but the application was not detected",
                manifest.display_name
            )),
            DesktopLifecycleAction::Update
                if !after.installed
                    || before.version.is_some()
                        && after.version.as_deref() == before.version.as_deref() =>
            {
                Err(format!(
                    "{} update completed but the installed version did not change",
                    manifest.display_name
                ))
            }
            DesktopLifecycleAction::Uninstall if after.installed => Err(format!(
                "{} uninstall completed but the package is still installed",
                manifest.display_name
            )),
            _ => Ok(after),
        }
    })
    .await
    .map_err(|error| format!("Desktop lifecycle task failed: {error}"))?
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
            let package_identity = status
                .package_identity
                .as_deref()
                .ok_or_else(|| "Desktop package identity is unavailable".to_string())?;
            let family_name = package_identity
                .split('_')
                .next()
                .and_then(|_| {
                    let script = format!(
                        "(Get-AppxPackage -Name '{}').PackageFamilyName",
                        manifest.windows_package_name
                    );
                    powershell_output(&script, "desktop package family lookup").ok()
                })
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "Desktop package family name is unavailable".to_string())?;
            let app_user_model_id = format!(
                "{}!{}",
                family_name.trim(),
                manifest.windows_app_id_suffix
            );
            let mut command = Command::new("explorer.exe");
            command.arg(format!("shell:AppsFolder\\{app_user_model_id}"));
            command
                .spawn()
                .map_err(|error| format!("Failed to launch {}: {error}", manifest.display_name))?;
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
