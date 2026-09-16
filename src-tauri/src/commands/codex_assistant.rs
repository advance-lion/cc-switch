//! Codex app-server transport for the in-app assistant.
//!
//! This module deliberately exposes conversation operations only. It does not
//! expose a generic shell command: Codex owns command/file execution and asks
//! the UI for approval through its native JSON-RPC server requests.

use once_cell::sync::Lazy;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

const EVENT_NAME: &str = "codex-assistant-event";
const MAX_INPUT_LENGTH: usize = 60_000;
const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_APPROVAL_METHOD: &str = "item/commandExecution/requestApproval";
const FILE_APPROVAL_METHOD: &str = "item/fileChange/requestApproval";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexAssistantApproval {
    id: String,
    #[serde(rename = "type")]
    approval_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    network_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grant_root: Option<String>,
    allow_for_session: bool,
    available_decisions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum CodexAssistantEvent {
    Started {
        session_id: String,
    },
    Message {
        session_id: String,
        message: String,
    },
    Log {
        session_id: String,
        message: String,
    },
    Stderr {
        session_id: String,
        message: String,
    },
    Disconnected {
        session_id: String,
        message: String,
    },
    Approval {
        session_id: String,
        approval: CodexAssistantApproval,
    },
    Finished {
        session_id: String,
        success: bool,
        cancelled: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct PendingApproval {
    request_id: Value,
    method: String,
    available_decisions: HashSet<String>,
    high_risk: bool,
}

trait CodexStdin: Write + Send {}
impl<T: Write + Send> CodexStdin for T {}

trait CodexEventSink: Send + Sync {
    fn emit(&self, event: CodexAssistantEvent);
}

struct TauriEventSink(AppHandle);

impl CodexEventSink for TauriEventSink {
    fn emit(&self, event: CodexAssistantEvent) {
        let _ = self.0.emit(EVENT_NAME, event);
    }
}

trait CodexProcess: Send {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;
    fn kill(&mut self) -> std::io::Result<()>;
    fn id(&self) -> u32;
}

impl CodexProcess for Child {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        Child::try_wait(self)
    }

    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }

    fn id(&self) -> u32 {
        Child::id(self)
    }
}

struct CodexAppServerSession {
    id: String,
    events: Arc<dyn CodexEventSink>,
    child: Mutex<Box<dyn CodexProcess>>,
    stdin: Mutex<Box<dyn CodexStdin>>,
    next_request_id: AtomicU64,
    pending_responses: Mutex<HashMap<String, mpsc::Sender<Result<Value, String>>>>,
    pending_approvals: Mutex<HashMap<String, PendingApproval>>,
    thread_id: Mutex<Option<String>>,
    active_turn_id: Mutex<Option<String>>,
    turn_active: AtomicBool,
    closing: AtomicBool,
}

static SESSIONS: Lazy<Mutex<HashMap<String, Arc<CodexAppServerSession>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn emit_event(events: &dyn CodexEventSink, event: CodexAssistantEvent) {
    events.emit(event);
}

fn validate_input(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("请输入消息".to_string());
    }
    if input.chars().count() > MAX_INPUT_LENGTH {
        return Err(format!("消息过长（最多 {MAX_INPUT_LENGTH} 个字符）"));
    }
    Ok(input.to_string())
}

fn resolve_workspace_dir(raw: Option<&str>) -> Result<PathBuf, String> {
    let path = match raw.map(str::trim).filter(|value| !value.is_empty()) {
        Some(raw) => fs::canonicalize(raw).map_err(|error| format!("无法访问工作目录: {error}"))?,
        None => {
            let path = crate::config::get_app_config_dir()
                .join("codex-assistant")
                .join("workspace");
            fs::create_dir_all(&path).map_err(|error| format!("无法创建助手工作目录: {error}"))?;
            fs::canonicalize(path).map_err(|error| format!("无法访问助手工作目录: {error}"))?
        }
    };
    if !path.is_dir() {
        return Err("工作目录必须是文件夹".to_string());
    }
    Ok(path)
}

/// Windows npm/pnpm installations expose Codex as a `.cmd` shim. Never pass
/// assistant input through cmd.exe; resolve the fixed official JS entry and
/// invoke Node with an argv array instead.
#[cfg(target_os = "windows")]
fn codex_assistant_command_for(executable: &Path) -> Result<Command, String> {
    let is_batch = executable
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        });
    if !is_batch {
        return Ok(Command::new(executable));
    }

    let bin_dir = executable
        .parent()
        .ok_or_else(|| "Codex CLI 批处理入口没有父目录".to_string())?;
    let entry = [
        bin_dir.join("node_modules/@openai/codex/bin/codex.js"),
        bin_dir.join("../@openai/codex/bin/codex.js"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .ok_or_else(|| {
        format!(
            "无法从 Codex CLI 入口 {} 定位官方 Node 启动文件",
            executable.display()
        )
    })?;
    let entry = super::misc::windows_shell_compatible_path(&entry);
    let local_node = bin_dir.join("node.exe");
    let node = if local_node.is_file() {
        super::misc::windows_shell_compatible_path(&local_node)
    } else {
        let node = super::misc::resolve_tool_executable_for_gui("node");
        super::misc::windows_shell_compatible_path(&node)
    };
    let mut command = Command::new(node);
    command.arg(entry);
    Ok(command)
}

#[cfg(not(target_os = "windows"))]
fn codex_assistant_command_for(executable: &Path) -> Result<Command, String> {
    Ok(Command::new(executable))
}

fn codex_assistant_home() -> PathBuf {
    let configured = crate::codex_config::get_codex_config_dir();
    if !crate::config::is_test_sandbox()
        || crate::settings::get_codex_override_dir().is_some()
        || configured.join("config.toml").is_file()
        || configured.join("auth.json").is_file()
    {
        return configured;
    }

    #[cfg(debug_assertions)]
    if let Some(real_home) = dirs::home_dir() {
        let host_codex = real_home.join(".codex");
        if host_codex.join("config.toml").is_file() || host_codex.join("auth.json").is_file() {
            return host_codex;
        }
    }

    configured
}

fn request_key(id: &Value) -> Result<String, String> {
    match id {
        Value::String(_) | Value::Number(_) => {
            serde_json::to_string(id).map_err(|error| format!("无效的 JSON-RPC 请求 ID: {error}"))
        }
        _ => Err("JSON-RPC 请求 ID 必须是字符串或数字".to_string()),
    }
}

fn response_error(value: &Value) -> String {
    value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("Codex app-server 请求失败")
        .to_string()
}

impl CodexAppServerSession {
    fn write_message(&self, message: &Value) -> Result<(), String> {
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| "Codex app-server 输入锁不可用".to_string())?;
        serde_json::to_writer(&mut *stdin, message)
            .map_err(|error| format!("无法编码 Codex app-server 消息: {error}"))?;
        stdin
            .write_all(b"\n")
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("无法写入 Codex app-server: {error}"))
    }

    fn notify(&self, method: &str, params: Option<Value>) -> Result<(), String> {
        let mut message = json!({ "method": method });
        if let Some(params) = params {
            message["params"] = params;
        }
        self.write_message(&message)
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = Value::from(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let key = request_key(&id)?;
        let (sender, receiver) = mpsc::channel();
        self.pending_responses
            .lock()
            .map_err(|_| "Codex app-server 响应锁不可用".to_string())?
            .insert(key.clone(), sender);

        if let Err(error) = self.write_message(&json!({
            "id": id,
            "method": method,
            "params": params,
        })) {
            if let Ok(mut pending) = self.pending_responses.lock() {
                pending.remove(&key);
            }
            return Err(error);
        }

        match receiver.recv_timeout(RPC_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(mut pending) = self.pending_responses.lock() {
                    pending.remove(&key);
                }
                Err(format!("Codex app-server 请求超时: {method}"))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("Codex app-server 已断开连接".to_string())
            }
        }
    }

    fn fail_pending_responses(&self, error: &str) {
        if let Ok(mut pending) = self.pending_responses.lock() {
            for (_, sender) in pending.drain() {
                let _ = sender.send(Err(error.to_string()));
            }
        }
    }
}

fn get_session(session_id: &str) -> Result<Arc<CodexAppServerSession>, String> {
    SESSIONS
        .lock()
        .map_err(|_| "Codex 会话状态锁不可用".to_string())?
        .get(session_id)
        .cloned()
        .ok_or_else(|| "Codex 会话不存在或已关闭".to_string())
}

fn extract_id(value: &Value, pointer: &str, label: &str) -> Result<String, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("Codex app-server 未返回 {label}"))
}

fn is_high_risk_command(command: &str) -> bool {
    let normalized = command
        .to_ascii_lowercase()
        .replace(['\r', '\n', '\t'], " ");
    let padded = format!(" {normalized} ");
    const HIGH_RISK_MARKERS: &[&str] = &[
        " rm ",
        " rm.exe ",
        " rmdir ",
        " del ",
        " erase ",
        " remove-item ",
        " format ",
        " diskpart ",
        " clean all ",
        " shutdown ",
        " restart-computer ",
        " stop-computer ",
        " reboot ",
        " reg delete ",
        " sc delete ",
        " net user ",
        " bcdedit ",
        " cipher /w ",
        " invoke-expression ",
        " iex ",
        " encodedcommand ",
        " --force ",
        " -force ",
        " reset --hard ",
        " clean -fd ",
        " clean -df ",
    ];
    HIGH_RISK_MARKERS
        .iter()
        .any(|marker| padded.contains(marker))
        || normalized.contains("curl ")
            && (normalized.contains("| sh") || normalized.contains("|sh"))
        || normalized.contains("wget ")
            && (normalized.contains("| sh") || normalized.contains("|sh"))
        || normalized.contains(":(){:|:&};:")
}

fn advertised_string_decisions(params: &Value) -> Option<HashSet<String>> {
    params
        .get("availableDecisions")
        .and_then(Value::as_array)
        .map(|decisions| {
            decisions
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
}

fn allowed_approval_decisions(method: &str, params: &Value, high_risk: bool) -> HashSet<String> {
    let whitelist: HashSet<String> = match method {
        COMMAND_APPROVAL_METHOD | FILE_APPROVAL_METHOD => {
            ["accept", "acceptForSession", "decline", "cancel"]
                .into_iter()
                .map(str::to_string)
                .collect()
        }
        _ => HashSet::new(),
    };

    let mut allowed: HashSet<String> = if let Some(advertised) = advertised_string_decisions(params)
    {
        whitelist.intersection(&advertised).cloned().collect()
    } else {
        ["accept", "cancel"]
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    if high_risk {
        allowed.remove("acceptForSession");
    }
    allowed
}

fn ordered_decisions(decisions: &HashSet<String>) -> Vec<String> {
    ["accept", "acceptForSession", "decline", "cancel"]
        .into_iter()
        .filter(|decision| decisions.contains(*decision))
        .map(str::to_string)
        .collect()
}

fn handle_server_request(session: &Arc<CodexAppServerSession>, message: &Value) {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    let Some(request_id) = message.get("id").cloned() else {
        return;
    };
    if request_key(&request_id).is_err() {
        return;
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

    if method != COMMAND_APPROVAL_METHOD && method != FILE_APPROVAL_METHOD {
        let _ = session.write_message(&json!({
            "id": request_id,
            "error": { "code": -32601, "message": "Unsupported app-server request" }
        }));
        return;
    }

    let command = params.get("command").and_then(Value::as_str).unwrap_or("");
    let high_risk = method == COMMAND_APPROVAL_METHOD && is_high_risk_command(command);
    let available_decisions = allowed_approval_decisions(method, &params, high_risk);
    if available_decisions.is_empty() {
        let _ = session.write_message(&json!({
            "id": request_id,
            "result": { "decision": "cancel" }
        }));
        emit_event(
            session.events.as_ref(),
            CodexAssistantEvent::Stderr {
                session_id: session.id.clone(),
                message: "Codex 请求了当前 CC Switch 无法安全呈现的审批类型，已取消该操作。"
                    .to_string(),
            },
        );
        return;
    }
    let pending = PendingApproval {
        request_id: request_id.clone(),
        method: method.to_string(),
        available_decisions: available_decisions.clone(),
        high_risk,
    };
    let approval_id = Uuid::new_v4().to_string();
    if let Ok(mut approvals) = session.pending_approvals.lock() {
        approvals.insert(approval_id.clone(), pending);
    } else {
        let _ = session.write_message(&json!({
            "id": request_id,
            "result": { "decision": "cancel" }
        }));
        return;
    }

    let decisions = ordered_decisions(&available_decisions);
    emit_event(
        session.events.as_ref(),
        CodexAssistantEvent::Approval {
            session_id: session.id.clone(),
            approval: CodexAssistantApproval {
                id: approval_id,
                approval_type: if method == COMMAND_APPROVAL_METHOD {
                    "command"
                } else {
                    "fileChange"
                }
                .to_string(),
                command: params
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                cwd: params
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                reason: params
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                network_host: params
                    .pointer("/networkApprovalContext/host")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                grant_root: params
                    .get("grantRoot")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                allow_for_session: decisions
                    .iter()
                    .any(|decision| decision == "acceptForSession"),
                available_decisions: decisions,
            },
        },
    );
}

fn handle_protocol_message(session: &Arc<CodexAppServerSession>, message: Value) {
    if let Some(id) = message.get("id") {
        if message.get("method").is_some() {
            handle_server_request(session, &message);
            return;
        }
        if let Ok(key) = request_key(id) {
            let sender = session
                .pending_responses
                .lock()
                .ok()
                .and_then(|mut pending| pending.remove(&key));
            if let Some(sender) = sender {
                let result = if message.get("error").is_some() {
                    Err(response_error(&message))
                } else {
                    Ok(message.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = sender.send(result);
            }
        }
        return;
    }

    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "item/agentMessage/delta" => {
            if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                emit_event(
                    session.events.as_ref(),
                    CodexAssistantEvent::Message {
                        session_id: session.id.clone(),
                        message: delta.to_string(),
                    },
                );
            }
        }
        "item/commandExecution/outputDelta" | "item/commandExecution/terminalInteraction" => {
            if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                emit_event(
                    session.events.as_ref(),
                    CodexAssistantEvent::Log {
                        session_id: session.id.clone(),
                        message: delta.to_string(),
                    },
                );
            }
        }
        "item/fileChange/outputDelta" | "item/fileChange/patchUpdated" => {
            emit_event(
                session.events.as_ref(),
                CodexAssistantEvent::Log {
                    session_id: session.id.clone(),
                    message: params.to_string(),
                },
            );
        }
        "turn/started" => {
            session.turn_active.store(true, Ordering::Release);
            if let Some(turn_id) = params
                .get("turnId")
                .or_else(|| params.pointer("/turn/id"))
                .and_then(Value::as_str)
            {
                if let Ok(mut active) = session.active_turn_id.lock() {
                    *active = Some(turn_id.to_string());
                }
            }
            emit_event(
                session.events.as_ref(),
                CodexAssistantEvent::Started {
                    session_id: session.id.clone(),
                },
            );
        }
        "turn/completed" => {
            session.turn_active.store(false, Ordering::Release);
            if let Ok(mut active) = session.active_turn_id.lock() {
                *active = None;
            }
            if let Ok(mut approvals) = session.pending_approvals.lock() {
                approvals.clear();
            }
            let status = params.pointer("/turn/status").and_then(Value::as_str);
            let success = matches!(status, Some("completed"));
            let cancelled = matches!(status, Some("interrupted" | "cancelled"));
            let error = params
                .pointer("/turn/error/message")
                .or_else(|| params.pointer("/turn/error"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    (!success && !cancelled).then(|| status.unwrap_or("Codex 回合失败").to_string())
                });
            emit_event(
                session.events.as_ref(),
                CodexAssistantEvent::Finished {
                    session_id: session.id.clone(),
                    success,
                    cancelled,
                    message: error,
                },
            );
        }
        "error" => {
            let error = params
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("Codex app-server 报告错误")
                .to_string();
            emit_event(
                session.events.as_ref(),
                CodexAssistantEvent::Stderr {
                    session_id: session.id.clone(),
                    message: error,
                },
            );
        }
        _ => {}
    }
}

fn start_protocol_readers(
    session: Arc<CodexAppServerSession>,
    stdout: impl std::io::Read + Send + 'static,
    stderr: impl std::io::Read + Send + 'static,
) {
    let stdout_session = session.clone();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => match serde_json::from_str::<Value>(&line) {
                    Ok(message) => handle_protocol_message(&stdout_session, message),
                    Err(error) => emit_event(
                        stdout_session.events.as_ref(),
                        CodexAssistantEvent::Stderr {
                            session_id: stdout_session.id.clone(),
                            message: format!("无法解析 Codex app-server 消息: {error}"),
                        },
                    ),
                },
                Ok(_) => {}
                Err(error) => {
                    emit_event(
                        stdout_session.events.as_ref(),
                        CodexAssistantEvent::Stderr {
                            session_id: stdout_session.id.clone(),
                            message: format!("读取 Codex app-server 输出失败: {error}"),
                        },
                    );
                    break;
                }
            }
        }
        stdout_session.fail_pending_responses("Codex app-server 已断开连接");
    });

    let stderr_session = session.clone();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            emit_event(
                stderr_session.events.as_ref(),
                CodexAssistantEvent::Stderr {
                    session_id: stderr_session.id.clone(),
                    message: line,
                },
            );
        }
    });

    thread::spawn(move || {
        loop {
            let status = match session.child.lock() {
                Ok(mut child) => child.try_wait(),
                Err(_) => return,
            };
            match status {
                Ok(Some(_)) => break,
                Ok(None) => thread::sleep(Duration::from_millis(100)),
                Err(_) => break,
            }
        }
        let turn_was_active = session.turn_active.swap(false, Ordering::AcqRel);
        if let Ok(mut approvals) = session.pending_approvals.lock() {
            approvals.clear();
        }
        session.fail_pending_responses("Codex app-server 已退出");
        if !session.closing.load(Ordering::Acquire) {
            let message = if turn_was_active {
                "Codex app-server 在回复过程中意外退出"
            } else {
                "Codex app-server 已意外断开，请重新发送消息以建立新会话"
            };
            log::error!(
                "Codex assistant session {} disconnected: {message}",
                session.id
            );
            emit_event(
                session.events.as_ref(),
                CodexAssistantEvent::Disconnected {
                    session_id: session.id.clone(),
                    message: message.to_string(),
                },
            );
        }
        if let Ok(mut sessions) = SESSIONS.lock() {
            sessions.remove(&session.id);
        }
    });
}

fn configure_app_server_command(command: &mut Command, codex_home: &Path) {
    command
        .arg("app-server")
        .arg("--stdio")
        // The assistant does not use image generation, and some compatible
        // Providers reject Codex's reserved image_gen tool schema.
        .arg("--disable")
        .arg("image_generation")
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

fn spawn_app_server(app: AppHandle) -> Result<Arc<CodexAppServerSession>, String> {
    let executable = super::misc::resolve_tool_executable_for_gui("codex");
    let mut command = codex_assistant_command_for(&executable)?;
    let codex_home = codex_assistant_home();
    fs::create_dir_all(&codex_home).map_err(|error| format!("无法创建 Codex 配置目录: {error}"))?;
    configure_app_server_command(&mut command, &codex_home);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 Codex app-server: {error}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "无法打开 Codex app-server 输入".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法读取 Codex app-server 输出".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取 Codex app-server 错误输出".to_string())?;
    let session = Arc::new(CodexAppServerSession {
        id: Uuid::new_v4().to_string(),
        events: Arc::new(TauriEventSink(app)),
        child: Mutex::new(Box::new(child)),
        stdin: Mutex::new(Box::new(stdin)),
        next_request_id: AtomicU64::new(1),
        pending_responses: Mutex::new(HashMap::new()),
        pending_approvals: Mutex::new(HashMap::new()),
        thread_id: Mutex::new(None),
        active_turn_id: Mutex::new(None),
        turn_active: AtomicBool::new(false),
        closing: AtomicBool::new(false),
    });
    SESSIONS
        .lock()
        .map_err(|_| "Codex 会话状态锁不可用".to_string())?
        .insert(session.id.clone(), session.clone());
    start_protocol_readers(session.clone(), stdout, stderr);
    Ok(session)
}

fn terminate_process(session: &CodexAppServerSession) {
    let Ok(mut child) = session.child.lock() else {
        return;
    };
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

fn initialize_session(session: &Arc<CodexAppServerSession>, cwd: &Path) -> Result<(), String> {
    session.request(
        "initialize",
        json!({
            "clientInfo": {
                "name": "cc-switch",
                "title": "CC Switch",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": { "experimentalApi": true }
        }),
    )?;
    session.notify("initialized", None)?;
    let result = session.request(
        "thread/start",
        json!({
            "cwd": cwd.to_string_lossy(),
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "sandbox": "read-only",
            "ephemeral": true,
            "developerInstructions": "You are the CC Switch in-app assistant. Inspect the actual machine with Codex native tools when needed instead of claiming you cannot inspect it. CC Switch has fixed lifecycle installers for Claude Code, Codex CLI, Gemini CLI, Grok CLI, OpenCode, OpenClaw, Hermes Agent, and Pi; recommend the Standard Install button as the fastest stable route for those tools, but if the user explicitly asks you to continue, investigate and proceed through native approvals. For unsupported agents, investigate their official installation method and request approval for any command, network access, or file change. Never claim an action succeeded until its tool result confirms it."
        }),
    )?;
    let thread_id = extract_id(&result, "/thread/id", "thread id")?;
    *session
        .thread_id
        .lock()
        .map_err(|_| "Codex thread 状态锁不可用".to_string())? = Some(thread_id);
    Ok(())
}

#[tauri::command]
pub fn start_codex_assistant_session(app: AppHandle) -> Result<String, String> {
    let cwd = resolve_workspace_dir(None)?;
    let session = spawn_app_server(app)?;
    if let Err(error) = initialize_session(&session, &cwd) {
        session.closing.store(true, Ordering::Release);
        terminate_process(&session);
        return Err(error);
    }
    Ok(session.id.clone())
}

#[tauri::command]
pub fn send_codex_assistant_message(session_id: String, message: String) -> Result<(), String> {
    let input = validate_input(&message)?;
    let session = get_session(&session_id)?;
    if session
        .turn_active
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("当前 Codex 回合尚未结束".to_string());
    }
    let result = (|| {
        let thread_id = session
            .thread_id
            .lock()
            .map_err(|_| "Codex thread 状态锁不可用".to_string())?
            .clone()
            .ok_or_else(|| "Codex thread 尚未初始化".to_string())?;
        let result = session.request(
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{ "type": "text", "text": input }]
            }),
        )?;
        let turn_id = extract_id(&result, "/turn/id", "turn id")?;
        if session.turn_active.load(Ordering::Acquire) {
            *session
                .active_turn_id
                .lock()
                .map_err(|_| "Codex turn 状态锁不可用".to_string())? = Some(turn_id);
        }
        Ok(())
    })();
    if result.is_err() {
        session.turn_active.store(false, Ordering::Release);
    }
    result
}

#[tauri::command]
pub fn respond_codex_assistant_approval(
    session_id: String,
    approval_id: String,
    decision: String,
) -> Result<bool, String> {
    let session = get_session(&session_id)?;
    let decision = decision.trim();
    let pending = {
        let mut approvals = session
            .pending_approvals
            .lock()
            .map_err(|_| "Codex 审批状态锁不可用".to_string())?;
        let pending = approvals
            .get(&approval_id)
            .cloned()
            .ok_or_else(|| "审批请求不存在或已处理".to_string())?;
        if !pending.available_decisions.contains(decision) {
            return Err(format!("当前审批不支持 decision: {decision}"));
        }
        if decision == "acceptForSession"
            && pending.method == COMMAND_APPROVAL_METHOD
            && pending.high_risk
        {
            return Err("高风险命令不能在本会话中持续授权".to_string());
        }
        approvals.remove(&approval_id);
        pending
    };
    if let Err(error) = session.write_message(&json!({
        "id": pending.request_id,
        "result": { "decision": decision }
    })) {
        if let Ok(mut approvals) = session.pending_approvals.lock() {
            approvals.insert(approval_id, pending);
        }
        return Err(error);
    }
    Ok(true)
}

#[tauri::command]
pub fn cancel_codex_assistant_run(session_id: String) -> Result<bool, String> {
    let session = get_session(&session_id)?;
    let thread_id = session
        .thread_id
        .lock()
        .map_err(|_| "Codex thread 状态锁不可用".to_string())?
        .clone()
        .ok_or_else(|| "Codex thread 尚未初始化".to_string())?;
    let turn_id = session
        .active_turn_id
        .lock()
        .map_err(|_| "Codex turn 状态锁不可用".to_string())?
        .clone()
        .ok_or_else(|| "没有正在运行的 Codex 回合".to_string())?;
    session.request(
        "turn/interrupt",
        json!({ "threadId": thread_id, "turnId": turn_id }),
    )?;
    Ok(true)
}

#[tauri::command]
pub fn close_codex_assistant_session(session_id: String) -> Result<bool, String> {
    let session = get_session(&session_id)?;
    if session.closing.swap(true, Ordering::AcqRel) {
        return Ok(true);
    }

    terminate_process(&session);
    if let Ok(mut sessions) = SESSIONS.lock() {
        sessions.remove(&session_id);
    }
    Ok(true)
}

pub fn shutdown_codex_assistant_sessions() {
    let sessions = SESSIONS
        .lock()
        .map(|mut sessions| {
            sessions
                .drain()
                .map(|(_, session)| session)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for session in sessions {
        session.closing.store(true, Ordering::Release);
        terminate_process(&session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor, ErrorKind, Read};

    static TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[derive(Clone, Default)]
    struct RecordingEventSink(Arc<Mutex<Vec<CodexAssistantEvent>>>);

    impl CodexEventSink for RecordingEventSink {
        fn emit(&self, event: CodexAssistantEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    struct FakeStdin {
        lines: mpsc::Sender<String>,
        buffer: Vec<u8>,
        fail_writes: Arc<AtomicBool>,
    }

    impl Write for FakeStdin {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_writes.load(Ordering::Acquire) {
                return Err(io::Error::new(ErrorKind::BrokenPipe, "fake stdin closed"));
            }
            self.buffer.extend_from_slice(bytes);
            while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line = String::from_utf8(self.buffer.drain(..=newline).collect())
                    .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
                self.lines
                    .send(line.trim_end().to_string())
                    .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "fake server stopped"))?;
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FakeStdout(mpsc::Receiver<Vec<u8>>);

    impl Read for FakeStdout {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let bytes = self
                .0
                .recv()
                .map_err(|_| io::Error::new(ErrorKind::UnexpectedEof, "fake stdout closed"))?;
            let count = bytes.len().min(buffer.len());
            buffer[..count].copy_from_slice(&bytes[..count]);
            if count != bytes.len() {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "fake stdout chunk too large",
                ));
            }
            Ok(count)
        }
    }

    #[derive(Default)]
    struct FakeProcess;

    impl CodexProcess for FakeProcess {
        fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
            Ok(None)
        }

        fn kill(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn id(&self) -> u32 {
            0
        }
    }

    #[derive(Clone)]
    struct FakeSession {
        session: Arc<CodexAppServerSession>,
        input: Arc<Mutex<mpsc::Receiver<String>>>,
        output: mpsc::Sender<Vec<u8>>,
        events: RecordingEventSink,
        fail_writes: Arc<AtomicBool>,
    }

    impl FakeSession {
        fn send(&self, message: Value) {
            self.output
                .send(format!("{message}\n").into_bytes())
                .unwrap();
        }

        fn receive(&self) -> Value {
            serde_json::from_str(
                &self
                    .input
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap(),
            )
            .unwrap()
        }
    }

    fn fake_session(id: &str) -> FakeSession {
        let (input_tx, input) = mpsc::channel();
        let (output, stdout) = mpsc::channel();
        let events = RecordingEventSink::default();
        let fail_writes = Arc::new(AtomicBool::new(false));
        let session = Arc::new(CodexAppServerSession {
            id: id.to_string(),
            events: Arc::new(events.clone()),
            child: Mutex::new(Box::new(FakeProcess)),
            stdin: Mutex::new(Box::new(FakeStdin {
                lines: input_tx,
                buffer: Vec::new(),
                fail_writes: fail_writes.clone(),
            })),
            next_request_id: AtomicU64::new(1),
            pending_responses: Mutex::new(HashMap::new()),
            pending_approvals: Mutex::new(HashMap::new()),
            thread_id: Mutex::new(None),
            active_turn_id: Mutex::new(None),
            turn_active: AtomicBool::new(false),
            closing: AtomicBool::new(false),
        });
        start_protocol_readers(session.clone(), FakeStdout(stdout), Cursor::new(Vec::new()));
        FakeSession {
            session,
            input: Arc::new(Mutex::new(input)),
            output,
            events,
            fail_writes,
        }
    }

    fn wait_for_event(events: &RecordingEventSink) -> CodexAssistantEvent {
        for _ in 0..100 {
            if let Some(event) = events.0.lock().unwrap().pop() {
                return event;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for fake Codex event");
    }

    fn insert_test_session(session: &Arc<CodexAppServerSession>) {
        SESSIONS
            .lock()
            .unwrap()
            .insert(session.id.clone(), session.clone());
    }

    fn remove_test_session(id: &str) {
        SESSIONS.lock().unwrap().remove(id);
    }

    #[test]
    fn fake_stdio_initialization_is_ordered_and_uses_fixed_safe_options() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fake = fake_session("initialization");
        let server_fake = fake.clone();
        let server = thread::spawn(move || {
            let initialize = server_fake.receive();
            let id = initialize["id"].clone();
            server_fake.send(json!({ "id": id, "result": {} }));
            let initialized = server_fake.receive();
            let thread_start = server_fake.receive();
            let id = thread_start["id"].clone();
            server_fake.send(json!({ "id": id, "result": { "thread": { "id": "thread-1" } } }));
            (initialize, initialized, thread_start)
        });
        let workspace = tempfile::tempdir().unwrap();
        initialize_session(&fake.session, workspace.path()).unwrap();
        let (initialize, initialized, thread_start) = server.join().unwrap();
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(initialized["method"], "initialized");
        assert_eq!(thread_start["method"], "thread/start");
        assert_eq!(thread_start["params"]["approvalPolicy"], "on-request");
        assert_eq!(thread_start["params"]["sandbox"], "read-only");
        assert_eq!(
            *fake.session.thread_id.lock().unwrap(),
            Some("thread-1".to_string())
        );
    }

    #[test]
    fn fake_stdio_approval_roundtrip_filters_persistent_high_risk_grants() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fake = fake_session("approval");
        insert_test_session(&fake.session);
        fake.send(json!({
            "id": "approval-1",
            "method": COMMAND_APPROVAL_METHOD,
            "params": {
                "command": "rm -rf ./build",
                "availableDecisions": ["accept", "acceptForSession", "decline", "cancel"]
            }
        }));
        let event = wait_for_event(&fake.events);
        let CodexAssistantEvent::Approval { approval, .. } = event else {
            panic!("expected approval event");
        };
        assert_eq!(
            approval.available_decisions,
            vec!["accept", "decline", "cancel"]
        );
        assert!(!approval.allow_for_session);
        assert!(respond_codex_assistant_approval(
            "approval".to_string(),
            approval.id.clone(),
            "acceptForSession".to_string()
        )
        .is_err());
        assert!(respond_codex_assistant_approval(
            "approval".to_string(),
            approval.id,
            "accept".to_string()
        )
        .unwrap());
        let response = fake.receive();
        assert_eq!(
            response,
            json!({ "id": "approval-1", "result": { "decision": "accept" } })
        );
        remove_test_session("approval");
    }

    #[test]
    fn fake_stdio_cancellation_sends_thread_and_turn_payload() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fake = fake_session("cancel");
        *fake.session.thread_id.lock().unwrap() = Some("thread-1".to_string());
        *fake.session.active_turn_id.lock().unwrap() = Some("turn-1".to_string());
        insert_test_session(&fake.session);
        let server = thread::spawn(move || {
            let interrupt = fake.receive();
            let id = interrupt["id"].clone();
            fake.send(json!({ "id": id, "result": {} }));
            interrupt
        });
        assert!(cancel_codex_assistant_run("cancel".to_string()).unwrap());
        let interrupt = server.join().unwrap();
        assert_eq!(interrupt["method"], "turn/interrupt");
        assert_eq!(
            interrupt["params"],
            json!({ "threadId": "thread-1", "turnId": "turn-1" })
        );
        remove_test_session("cancel");
    }

    #[test]
    fn fake_stdio_write_failure_removes_pending_request() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fake = fake_session("write-failure");
        fake.fail_writes.store(true, Ordering::Release);
        assert!(fake.session.request("initialize", json!({})).is_err());
        assert!(fake.session.pending_responses.lock().unwrap().is_empty());
    }

    #[test]
    fn request_ids_preserve_number_zero_and_string_identity() {
        assert_eq!(request_key(&json!(0)).unwrap(), "0");
        assert_eq!(request_key(&json!("0")).unwrap(), "\"0\"");
        assert_ne!(
            request_key(&json!(0)).unwrap(),
            request_key(&json!("0")).unwrap()
        );
        assert!(request_key(&Value::Null).is_err());
    }

    #[test]
    fn command_decisions_are_intersected_with_server_choices() {
        let params = json!({
            "availableDecisions": [
                "accept",
                { "acceptWithExecpolicyAmendment": { "execpolicy_amendment": ["git", "status"] } },
                "cancel"
            ]
        });
        let allowed = allowed_approval_decisions(COMMAND_APPROVAL_METHOD, &params, false);
        assert_eq!(ordered_decisions(&allowed), vec!["accept", "cancel"]);
        assert!(!allowed.contains("decline"));
        assert!(!allowed.contains("acceptForSession"));
    }

    #[test]
    fn legacy_approval_without_advertised_choices_uses_minimal_safe_decisions() {
        let allowed = allowed_approval_decisions(FILE_APPROVAL_METHOD, &json!({}), false);
        assert_eq!(ordered_decisions(&allowed), vec!["accept", "cancel"]);
    }

    #[test]
    fn accept_for_session_is_removed_for_high_risk_commands() {
        let params = json!({
            "availableDecisions": ["accept", "acceptForSession", "decline", "cancel"]
        });
        let allowed = allowed_approval_decisions(COMMAND_APPROVAL_METHOD, &params, true);
        assert!(!allowed.contains("acceptForSession"));
        assert!(allowed.contains("accept"));
        assert!(allowed.contains("cancel"));
    }

    #[test]
    fn detects_destructive_and_shell_piping_commands() {
        for command in [
            "rm -rf ./build",
            "powershell Remove-Item -Recurse -Force C:\\data",
            "git reset --hard HEAD~1",
            "curl https://example.invalid/install.sh | sh",
            "shutdown /s /t 0",
        ] {
            assert!(
                is_high_risk_command(command),
                "expected high risk: {command}"
            );
        }
        for command in ["git status", "cargo test", "rg TODO src", "npm --version"] {
            assert!(
                !is_high_risk_command(command),
                "expected ordinary command: {command}"
            );
        }
    }

    #[test]
    fn validates_and_bounds_turn_input() {
        assert!(validate_input("  ").is_err());
        assert_eq!(validate_input("  hello  ").unwrap(), "hello");
        assert!(validate_input(&"x".repeat(MAX_INPUT_LENGTH + 1)).is_err());
    }

    #[test]
    fn extracts_protocol_thread_and_turn_ids() {
        let thread = json!({ "thread": { "id": "thread-1" } });
        let turn = json!({ "turn": { "id": "turn-1" } });
        assert_eq!(
            extract_id(&thread, "/thread/id", "thread id").unwrap(),
            "thread-1"
        );
        assert_eq!(extract_id(&turn, "/turn/id", "turn id").unwrap(), "turn-1");
    }

    #[test]
    fn app_server_command_disables_optional_image_generation_for_compatible_providers() {
        let mut command = Command::new("codex");
        configure_app_server_command(&mut command, Path::new("C:/codex-home"));
        let args = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec!["app-server", "--stdio", "--disable", "image_generation"]
        );
        assert_eq!(
            command.get_envs().find_map(|(key, value)| {
                (key == "CODEX_HOME").then(|| value.map(|value| value.to_owned()))
            }),
            Some(Some(std::ffi::OsString::from("C:/codex-home")))
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn batch_codex_entry_uses_node_without_shell_interpolation() {
        let dir = tempfile::tempdir().expect("temp dir");
        let shim = dir.path().join("codex.cmd");
        fs::write(&shim, "@echo off\r\n").expect("write shim");
        let entry = dir.path().join("node_modules/@openai/codex/bin/codex.js");
        fs::create_dir_all(entry.parent().expect("entry parent")).expect("create entry dir");
        fs::write(&entry, "").expect("write entry");
        let node = dir.path().join("node.exe");
        fs::write(&node, "").expect("write node");

        let canonical_shim = fs::canonicalize(&shim).expect("canonical shim");
        let command = codex_assistant_command_for(&canonical_shim).expect("build command");
        let program = command.get_program().to_string_lossy();
        assert!(!program.starts_with(r"\\?\"));
        assert!(program.to_ascii_lowercase().ends_with(r"\node.exe"));
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args.len(), 1);
        assert!(!args[0].starts_with(r"\\?\"));
        assert!(args[0]
            .replace('/', "\\")
            .to_ascii_lowercase()
            .ends_with(r"\node_modules\@openai\codex\bin\codex.js"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn executable_codex_entry_stays_direct() {
        let executable = Path::new(r"C:\tools\codex.exe");
        let command = codex_assistant_command_for(executable).expect("build command");
        assert_eq!(command.get_program(), executable.as_os_str());
        assert_eq!(command.get_args().count(), 0);
    }
}
