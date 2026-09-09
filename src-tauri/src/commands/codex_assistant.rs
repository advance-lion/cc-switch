//! Guarded Codex CLI automation for the in-app assistant.
//!
//! The assistant never exposes a generic shell endpoint. A user request first
//! produces a read-only, structured plan. For a CC Switch-supported app, the
//! approved plan is then executed only through a pre-registered lifecycle
//! action; Codex never receives workspace-write permission.

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

const EVENT_NAME: &str = "codex-assistant-event";
const MAX_REQUEST_LENGTH: usize = 6_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAssistantPlan {
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub steps: Vec<CodexAssistantPlanStep>,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default)]
    pub executable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<CodexAssistantInstallSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAssistantInstallSummary {
    pub tool: String,
    pub display_name: String,
    pub version: String,
    pub install_location: String,
    pub uses_default_location: bool,
    pub official_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAssistantPlanStep {
    pub label: String,
    pub description: String,
    #[serde(default)]
    pub requires_network: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexAssistantEvent {
    run_id: String,
    kind: String,
    message: Option<String>,
    plan_id: Option<String>,
    plan: Option<CodexAssistantPlan>,
    success: Option<bool>,
}

#[derive(Clone)]
struct StoredPlan {
    install: Option<RegisteredAssistantAction>,
}

#[derive(Clone)]
enum RegisteredAssistantAction {
    Cli(super::misc::RegisteredAssistantInstall),
    Desktop(super::desktop_lifecycle::RegisteredDesktopAssistantInstall),
}

impl RegisteredAssistantAction {
    fn tool(&self) -> &str {
        match self {
            Self::Cli(action) => &action.tool,
            Self::Desktop(action) => &action.app_id,
        }
    }

    fn display_name(&self) -> &str {
        match self {
            Self::Cli(action) => &action.display_name,
            Self::Desktop(action) => &action.display_name,
        }
    }

    fn version(&self) -> &str {
        match self {
            Self::Cli(action) => &action.version,
            Self::Desktop(action) => &action.version,
        }
    }

    fn install_location(&self) -> String {
        match self {
            Self::Cli(action) => action
                .install_dir
                .as_deref()
                .map(|directory| directory.display().to_string())
                .unwrap_or_else(|| "CC Switch 默认安装位置".to_string()),
            Self::Desktop(_) => "系统默认应用位置".to_string(),
        }
    }

    fn uses_default_location(&self) -> bool {
        match self {
            Self::Cli(action) => action.install_dir.is_none(),
            Self::Desktop(_) => true,
        }
    }

    fn official_source(&self) -> &str {
        match self {
            Self::Cli(action) => &action.official_source,
            Self::Desktop(action) => &action.official_source,
        }
    }
}

static RUNNING_PROCESSES: Lazy<Mutex<HashMap<String, Arc<Mutex<Child>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static PENDING_PLANS: Lazy<Mutex<HashMap<String, StoredPlan>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn emit_event(
    app: &AppHandle,
    run_id: &str,
    kind: &str,
    message: Option<String>,
    plan_id: Option<String>,
    plan: Option<CodexAssistantPlan>,
    success: Option<bool>,
) {
    let _ = app.emit(
        EVENT_NAME,
        CodexAssistantEvent {
            run_id: run_id.to_string(),
            kind: kind.to_string(),
            message,
            plan_id,
            plan,
            success,
        },
    );
}

fn validate_request(request: &str) -> Result<String, String> {
    let request = request.trim();
    if request.is_empty() {
        return Err("请输入需要安装或配置的 Agent".to_string());
    }
    if request.len() > MAX_REQUEST_LENGTH {
        return Err(format!("请求过长（最多 {MAX_REQUEST_LENGTH} 个字符）"));
    }
    Ok(request.to_string())
}

fn resolve_target_dir(raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("请先选择安装目录".to_string());
    }
    let path = fs::canonicalize(raw).map_err(|e| format!("无法访问安装目录: {e}"))?;
    if !path.is_dir() {
        return Err("安装位置必须是文件夹".to_string());
    }
    if path.parent().is_none() {
        return Err("不能将系统磁盘根目录作为 AI 安装工作区".to_string());
    }
    if dirs::home_dir().is_some_and(|home| home == path) {
        return Err("不能将整个用户主目录作为 AI 安装工作区".to_string());
    }
    Ok(path)
}

/// Codex needs a real directory for its read-only planning context even when
/// the user chooses the app's standard install location. Keep that internal
/// workspace separate from the actual installer target.
fn default_plan_workspace_dir() -> Result<PathBuf, String> {
    let path = crate::config::get_app_config_dir()
        .join("codex-assistant")
        .join("plan-workspace");
    fs::create_dir_all(&path).map_err(|error| format!("无法创建安装计划工作区: {error}"))?;
    fs::canonicalize(path).map_err(|error| format!("无法访问安装计划工作区: {error}"))
}

fn plan_schema_path() -> Result<PathBuf, String> {
    let path = crate::config::get_app_config_dir()
        .join("codex-assistant")
        .join("install-plan.schema.json");
    let parent = path
        .parent()
        .ok_or_else(|| "无法创建安装计划 schema 目录".to_string())?;
    fs::create_dir_all(parent).map_err(|e| format!("无法创建安装计划目录: {e}"))?;
    let schema = r#"{
  "type": "object",
  "additionalProperties": false,
  "required": ["title", "summary", "sources", "steps", "limitations"],
  "properties": {
    "title": { "type": "string" },
    "summary": { "type": "string" },
    "sources": { "type": "array", "items": { "type": "string" } },
    "limitations": { "type": "array", "items": { "type": "string" } },
    "steps": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["label", "description", "requiresNetwork"],
        "properties": {
          "label": { "type": "string" },
          "description": { "type": "string" },
          "requiresNetwork": { "type": "boolean" }
        }
      }
    }
  }
}"#;
    fs::write(&path, schema).map_err(|e| format!("无法写入安装计划 schema: {e}"))?;
    Ok(path)
}

fn build_plan_prompt(
    request: &str,
    _target_dir: &Path,
    install: Option<&RegisteredAssistantAction>,
) -> String {
    let registered = install.map_or_else(
        || {
            "This request is not a CC Switch-registered install action. You may only explain safe manual next steps; do not present it as executable.".to_string()
        },
        |action| {
            format!(
                "This is a CC Switch-registered installation for {}. The backend, not you, will execute its fixed installer after confirmation.\nUse only this official source in the explanation: {}. The selected installation location is: {}.\nRequested version channel: {}. Do not suggest another installer, package, URL, or shell command.",
                action.display_name(),
                action.official_source(),
                action.install_location(),
                action.version(),
            )
        },
    );
    format!(
        "You are CC Switch's installation planner. Do not install, download, modify files, or run shell commands. \n\
Create a concise installation explanation for the user's request. \n\
{} \n\
If reliable source or install instructions cannot be established, say so in limitations and propose a manual next step. \n\
Never propose deleting existing files, changing CC Switch configuration, changing Codex configuration, elevation, or disabling safety controls. \n\
Return only the JSON object required by the output schema.\n\nUser request:\n{}",
        registered,
        request
    )
}

fn parse_plan_response(text: &str) -> Option<CodexAssistantPlan> {
    let plan = serde_json::from_str::<CodexAssistantPlan>(text).ok()?;
    (!plan.title.trim().is_empty() && !plan.summary.trim().is_empty()).then_some(plan)
}

fn enrich_plan(
    mut plan: CodexAssistantPlan,
    install: Option<&RegisteredAssistantAction>,
) -> CodexAssistantPlan {
    if let Some(action) = install {
        plan.executable = true;
        plan.install = Some(CodexAssistantInstallSummary {
            tool: action.tool().to_string(),
            display_name: action.display_name().to_string(),
            version: action.version().to_string(),
            install_location: action.install_location(),
            uses_default_location: action.uses_default_location(),
            official_source: action.official_source().to_string(),
        });
    } else {
        plan.executable = false;
        plan.limitations.push(
            "该应用尚未登记受控安装器；可查看建议，但不能由 CC Switch 自动执行。".to_string(),
        );
    }
    plan
}

fn plan_output_path(run_id: &str) -> Result<PathBuf, String> {
    let schema_path = plan_schema_path()?;
    let parent = schema_path
        .parent()
        .ok_or_else(|| "无法创建安装计划输出目录".to_string())?;
    Ok(parent.join(format!("plan-output-{run_id}.json")))
}

fn spawn_codex_run(
    app: AppHandle,
    target_dir: PathBuf,
    prompt: String,
    plan_install: Option<RegisteredAssistantAction>,
) -> Result<String, String> {
    let run_id = Uuid::new_v4().to_string();
    let plan_output = plan_output_path(&run_id)?;
    let mut command = Command::new(super::misc::resolve_tool_executable_for_gui("codex"));
    command
        .arg("exec")
        .arg("--json")
        .arg("--ephemeral")
        .arg("--skip-git-repo-check")
        .arg("--ignore-rules")
        .arg("--cd")
        .arg(&target_dir)
        .arg("--sandbox")
        // Codex is deliberately restricted to planning. Installation is
        // performed by the registered CC Switch action, never by Codex.
        .arg("read-only")
        .arg("--output-schema")
        .arg(plan_schema_path()?)
        .arg("--output-last-message")
        .arg(&plan_output);
    command.arg(prompt);
    command
        .env("CODEX_HOME", crate::codex_config::get_codex_config_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    let child = command
        .spawn()
        .map_err(|e| format!("无法启动 Codex CLI：{e}"))?;
    let child = Arc::new(Mutex::new(child));
    let stdout = child
        .lock()
        .map_err(|_| "Codex 进程锁不可用".to_string())?
        .stdout
        .take()
        .ok_or_else(|| "无法读取 Codex CLI 输出".to_string())?;
    let stderr = child
        .lock()
        .map_err(|_| "Codex 进程锁不可用".to_string())?
        .stderr
        .take()
        .ok_or_else(|| "无法读取 Codex CLI 错误输出".to_string())?;

    RUNNING_PROCESSES
        .lock()
        .map_err(|_| "Codex 运行状态锁不可用".to_string())?
        .insert(run_id.clone(), child.clone());
    emit_event(
        &app,
        &run_id,
        "started",
        Some("plan".to_string()),
        None,
        None,
        None,
    );

    let stdout_run_id = run_id.clone();
    let stdout_app = app.clone();
    let stdout_thread = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            emit_event(
                &stdout_app,
                &stdout_run_id,
                "log",
                Some(line.clone()),
                None,
                None,
                None,
            );
        }
    });

    let stderr_run_id = run_id.clone();
    let stderr_app = app.clone();
    let stderr_thread = thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            emit_event(
                &stderr_app,
                &stderr_run_id,
                "stderr",
                Some(line),
                None,
                None,
                None,
            );
        }
    });

    let monitor_run_id = run_id.clone();
    thread::spawn(move || {
        let status = loop {
            let status = child
                .lock()
                .ok()
                .and_then(|mut process| process.try_wait().ok().flatten());
            if let Some(status) = status {
                break status;
            }
            thread::sleep(Duration::from_millis(125));
        };
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        if let Ok(mut running) = RUNNING_PROCESSES.lock() {
            running.remove(&monitor_run_id);
        }
        if status.success() {
            let response = fs::read_to_string(&plan_output).ok();
            let _ = fs::remove_file(&plan_output);
            if let Some(plan) = response.as_deref().and_then(parse_plan_response) {
                let plan_id = Uuid::new_v4().to_string();
                if let Ok(mut pending) = PENDING_PLANS.lock() {
                    let plan = enrich_plan(plan, plan_install.as_ref());
                    pending.insert(
                        plan_id.clone(),
                        StoredPlan {
                            install: plan_install,
                        },
                    );
                    emit_event(
                        &app,
                        &monitor_run_id,
                        "plan",
                        None,
                        Some(plan_id),
                        Some(plan),
                        None,
                    );
                }
            } else {
                emit_event(
                    &app,
                    &monitor_run_id,
                    "stderr",
                    Some("Codex 未返回可用的结构化安装计划".to_string()),
                    None,
                    None,
                    None,
                );
            }
        } else {
            let _ = fs::remove_file(plan_output);
        }
        emit_event(
            &app,
            &monitor_run_id,
            "finished",
            status.code().map(|code| format!("exit code: {code}")),
            None,
            None,
            Some(status.success()),
        );
    });

    Ok(run_id)
}

/// Executes a previously stored, CC Switch-registered action. The Codex CLI is
/// deliberately not involved here: its job ends after explaining the plan.
fn spawn_registered_install_run(
    app: AppHandle,
    install: RegisteredAssistantAction,
) -> Result<String, String> {
    let run_id = Uuid::new_v4().to_string();
    let display_name = install.display_name().to_string();
    let (child, cleanup_file) = match &install {
        RegisteredAssistantAction::Cli(action) => {
            super::misc::spawn_registered_assistant_install(action)?.into_parts()
        }
        RegisteredAssistantAction::Desktop(action) => (
            super::desktop_lifecycle::spawn_registered_desktop_install(action)?,
            None,
        ),
    };
    let child = Arc::new(Mutex::new(child));
    let stdout = child
        .lock()
        .map_err(|_| "安装进程锁不可用".to_string())?
        .stdout
        .take()
        .ok_or_else(|| "无法读取安装进程输出".to_string())?;
    let stderr = child
        .lock()
        .map_err(|_| "安装进程锁不可用".to_string())?
        .stderr
        .take()
        .ok_or_else(|| "无法读取安装进程错误输出".to_string())?;

    RUNNING_PROCESSES
        .lock()
        .map_err(|_| "Codex 运行状态锁不可用".to_string())?
        .insert(run_id.clone(), child.clone());
    emit_event(
        &app,
        &run_id,
        "started",
        Some(format!("install: {display_name}")),
        None,
        None,
        None,
    );

    let stdout_run_id = run_id.clone();
    let stdout_app = app.clone();
    let stdout_thread = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            emit_event(
                &stdout_app,
                &stdout_run_id,
                "log",
                Some(line),
                None,
                None,
                None,
            );
        }
    });
    let stderr_run_id = run_id.clone();
    let stderr_app = app.clone();
    let stderr_thread = thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            emit_event(
                &stderr_app,
                &stderr_run_id,
                "stderr",
                Some(line),
                None,
                None,
                None,
            );
        }
    });

    let monitor_run_id = run_id.clone();
    thread::spawn(move || {
        let status = loop {
            let status = child
                .lock()
                .ok()
                .and_then(|mut process| process.try_wait().ok().flatten());
            if let Some(status) = status {
                break status;
            }
            thread::sleep(Duration::from_millis(125));
        };
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        if let Some(path) = cleanup_file {
            let _ = fs::remove_file(path);
        }
        if let Ok(mut running) = RUNNING_PROCESSES.lock() {
            running.remove(&monitor_run_id);
        }

        let mut success = status.success();
        let mut message = status.code().map(|code| format!("exit code: {code}"));
        if success {
            emit_event(
                &app,
                &monitor_run_id,
                "log",
                Some("安装命令已结束，正在重新检测版本…".to_string()),
                None,
                None,
                None,
            );
            let verification = match &install {
                RegisteredAssistantAction::Cli(action) => {
                    super::misc::verify_registered_assistant_install(action)
                }
                RegisteredAssistantAction::Desktop(action) => {
                    super::desktop_lifecycle::verify_registered_desktop_install(action)
                }
            };
            match verification {
                Ok(version) => {
                    message = Some(format!(
                        "{} 已安装并验证：{}",
                        install.display_name(), version
                    ));
                    emit_event(
                        &app,
                        &monitor_run_id,
                        "log",
                        message.clone(),
                        None,
                        None,
                        None,
                    );
                }
                Err(error) => {
                    success = false;
                    message = Some(error.clone());
                    emit_event(
                        &app,
                        &monitor_run_id,
                        "stderr",
                        Some(error),
                        None,
                        None,
                        None,
                    );
                }
            }
        }
        emit_event(
            &app,
            &monitor_run_id,
            "finished",
            message,
            None,
            None,
            Some(success),
        );
    });

    Ok(run_id)
}

#[tauri::command]
pub fn start_codex_assistant_plan(
    app: AppHandle,
    request: String,
    target_dir: String,
    tool: Option<String>,
    requested_version: Option<String>,
    custom_install_location: Option<bool>,
) -> Result<String, String> {
    let request = validate_request(&request)?;
    let use_custom_location = custom_install_location.unwrap_or(true);
    let target_dir = if tool.is_some() && !use_custom_location {
        default_plan_workspace_dir()?
    } else {
        resolve_target_dir(&target_dir)?
    };
    let install = tool.as_deref().map(|tool| {
        if matches!(tool, "codex-desktop" | "claude-desktop") {
            return super::desktop_lifecycle::plan_registered_desktop_install(
                tool,
                requested_version.as_deref().unwrap_or("stable"),
                use_custom_location,
            )
            .map(RegisteredAssistantAction::Desktop);
        }
            // A dev build may perform a real test install, but its destination
            // is fixed by the backend under CC_SWITCH_TEST_HOME.  It never
            // honours a global/default destination while sandboxed.
            let sandbox_dir = crate::config::is_test_sandbox()
                .then(|| super::misc::sandbox_managed_install_dir(tool))
                .transpose()?;
            super::misc::plan_registered_assistant_install(
                tool,
                sandbox_dir
                    .as_deref()
                    .or_else(|| use_custom_location.then_some(target_dir.as_path())),
                requested_version.as_deref().unwrap_or("stable"),
            )
            .map(RegisteredAssistantAction::Cli)
        })
        .transpose()?;
    let prompt = build_plan_prompt(&request, &target_dir, install.as_ref());
    spawn_codex_run(app, target_dir, prompt, install)
}

#[tauri::command]
pub fn execute_codex_assistant_plan(app: AppHandle, plan_id: String) -> Result<String, String> {
    let stored = PENDING_PLANS
        .lock()
        .map_err(|_| "Codex 计划锁不可用".to_string())?
        .remove(&plan_id)
        .ok_or_else(|| "安装计划不存在或已失效，请重新生成".to_string())?;
    let install = stored
        .install
        .ok_or_else(|| "该计划没有已登记的受控安装器，只能查看建议和复制手动命令".to_string())?;
    spawn_registered_install_run(app, install)
}

#[tauri::command]
pub fn cancel_codex_assistant_run(run_id: String) -> Result<bool, String> {
    let child = RUNNING_PROCESSES
        .lock()
        .map_err(|_| "Codex 运行状态锁不可用".to_string())?
        .get(&run_id)
        .cloned()
        .ok_or_else(|| "没有正在运行的 Codex 任务".to_string())?;
    let child = child.lock().map_err(|_| "Codex 进程锁不可用".to_string())?;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x08000000)
            .status();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut child = child;
        let _ = child.kill();
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::parse_plan_response;

    #[test]
    fn parses_schema_plan_from_last_message() {
        let event = r#"{"title":"Install","summary":"Summary","sources":[],"steps":[{"label":"Download","description":"From official source","requiresNetwork":true}],"limitations":[]}"#;
        let plan = parse_plan_response(event).expect("plan should parse");
        assert_eq!(plan.title, "Install");
        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn rejects_incomplete_plan_response() {
        assert!(parse_plan_response(
            r#"{"title":"","summary":"","sources":[],"steps":[],"limitations":[]}"#
        )
        .is_none());
    }
}
