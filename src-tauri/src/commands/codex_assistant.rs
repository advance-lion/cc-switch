//! Codex exec transport for the in-app assistant.
//!
//! Runs `codex exec --json` as a subprocess per message. Each turn spawns a
//! fresh process that exits when complete, so no persistent locks or MCP
//! conflicts with the Codex desktop app. Uses the real CODEX_HOME so MCP
//! servers, hooks, and skills all work normally.

use once_cell::sync::Lazy;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
   sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

const EVENT_NAME: &str = "codex-assistant-event";
const MAX_INPUT_LENGTH: usize = 60_000;

/// Project knowledge injected as AGENTS.md into the Codex assistant workspace.
/// Codex CLI automatically reads AGENTS.md from its working directory on
/// startup, giving the assistant context about CC Switch Dev itself.
const AGENTS_MD: &str = r###"
# CC Switch Dev - Project Guide

## Overview
CC Switch Dev is an all-in-one desktop assistant for managing AI coding tools
(Claude Code, Codex CLI, Gemini CLI). Built with Tauri v2 (Rust + React/TS).

## Tech Stack
- Backend: Rust, Tauri v2, SQLite (rusqlite)
- Frontend: React 18, TypeScript, Tailwind CSS, Vite
- API bridge: Tauri invoke() commands + event listeners
- i18n: react-i18next (zh / en)

## Architecture

### Rust Backend (src-tauri/src/)
- commands/ -- Tauri command handlers (each file = one feature domain)
- database/ -- SQLite schema, migrations, queries
- proxy/ -- HTTP reverse proxy for API routing and failover
- provider_center/ -- Shared provider definitions and app adapters
- services/ -- Background services (usage sync, session import)
- config.rs -- App config dir resolution, global state

### Frontend (src/)
- components/providers/ -- Provider list, add/edit, switching UI
- components/provider-center/ -- Shared provider definition management
- components/assistant/CodexAssistantDock.tsx -- This assistant chat UI
- components/settings/ -- App settings panels
- components/runtime/ -- CLI tool install/update/launch UI
- components/hermes/ -- Hermes desktop integration
- components/mcp/ -- MCP server management
- components/sessions/ -- Session history browser
- components/usage/ -- Usage tracking and charts
- lib/api/ -- Tauri invoke wrappers (settings.ts, index.ts, etc.)
- contexts/ -- React contexts (AppConfig, Toast, etc.)

## Key Feature Domains

### Provider System (commands/provider.rs)
Manages API providers for Claude Code, Codex CLI, Gemini CLI, and others.
- CRUD: add_provider, update_provider, delete_provider, get_providers
- Switching: switch_provider (writes to live config files)
- Live config: reads/writes ~/.claude/settings.json, ~/.codex/config.toml, etc.
- Each provider has: name, app, base_url, api_key, model, headers

### Provider Center (commands/provider_center.rs)
Shared provider definitions that can be imported into multiple apps.
- Definitions are app-agnostic; bindings map them to specific apps
- Model catalog discovery via API probing

### Codex Assistant (commands/codex_assistant.rs)
The feature you are running inside right now.
- Uses codex exec --json as backend (one process per message)
- First message: codex exec --json -s danger-full-access -C <cwd> -
- Resume: codex exec resume <thread_id> --json --dangerously-bypass-approvals-and-sandbox -
- Uses real ~/.codex as CODEX_HOME (MCP, hooks, skills intact)
- Events stream via Tauri event: codex-assistant-event
- Generation counter prevents stale stdout readers from racing

### Desktop Lifecycle (commands/desktop_lifecycle.rs)
Manages desktop apps (Hermes, Codex Desktop, DeepSeek Harness).
- Install, update, launch, restart, uninstall
- Async jobs with cancellation support

### CLI Lifecycle (commands/cli_lifecycle.rs)
Manages CLI tools (codex, claude, gemini).
- Install, update, launch terminal, uninstall

### Proxy (commands/proxy.rs, proxy/)
HTTP reverse proxy for routing API calls through CC Switch.
- Failover across multiple providers
- Rate limiting and usage tracking

### MCP (commands/mcp.rs)
Model Context Protocol server configuration management.

### Session Manager (commands/session_manager.rs)
Imports and browses session history from CLI tools.

### Usage Tracking (commands/usage.rs)
Tracks API token usage per provider.

## Database
SQLite at <app_config_dir>/ccswitch.db. Key tables:
- providers, provider_bindings, model_pricing
- usage_events, sessions
- profiles, settings

## How to Add a New Provider
1. User adds via UI (Provider Center or per-app provider list)
2. Frontend calls add_provider Tauri command
3. Backend stores in SQLite + writes to live config file
4. switch_provider updates the app live config (e.g. ~/.codex/config.toml)

## Config File Locations
- App config: <APPDATA>/com.ccswitch.desktop.dev/
- Codex home: ~/.codex/ (config.toml, auth.json)
- Claude config: ~/.claude/settings.json
- CC Switch DB: <app_config_dir>/ccswitch.db

## Conventions
- Rust commands registered in lib.rs invoke_handler
- Frontend API wrappers in src/lib/api/settings.ts
- Events use Tauri emit() / listen()
- Chinese is the primary UI language; English i18n keys in src/i18n/
"###;

/// Write AGENTS.md to the workspace directory so Codex CLI reads it
/// automatically on startup and gains project context.
fn ensure_agents_md(dir: &Path) {
    let agents_path = dir.join("AGENTS.md");
    if let Ok(existing) = fs::read_to_string(&agents_path) {
        if existing == AGENTS_MD {
            return;
        }
    }
    if let Err(e) = fs::write(&agents_path, AGENTS_MD) {
        log::warn!("[codex-assistant] failed to write AGENTS.md: {e}");
    }
}


#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum CodexAssistantEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        session_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Message {
        session_id: String,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    Log {
        session_id: String,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    Stderr {
        session_id: String,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    Disconnected {
        session_id: String,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    Finished {
        session_id: String,
        success: bool,
        cancelled: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

trait CodexEventSink: Send + Sync {
    fn emit(&self, event: CodexAssistantEvent);
}

struct TauriEventSink(AppHandle);

impl CodexEventSink for TauriEventSink {
    fn emit(&self, event: CodexAssistantEvent) {
        let _ = self.0.emit(EVENT_NAME, event);
    }
}

fn emit_event(events: &dyn CodexEventSink, event: CodexAssistantEvent) {
    events.emit(event);
}

struct CodexExecSession {
    id: String,
    events: Arc<dyn CodexEventSink>,
    child: Mutex<Option<Child>>,
    thread_id: Mutex<Option<String>>,
    cwd: PathBuf,
    running: AtomicBool,
    turn_completed: AtomicBool,
    cancelled: AtomicBool,
    generation: AtomicU64,
}

static SESSIONS: Lazy<Mutex<HashMap<String, Arc<CodexExecSession>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

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
            ensure_agents_md(&path);
            fs::canonicalize(path).map_err(|error| format!("无法访问助手工作目录: {error}"))?
        }
    };
    if !path.is_dir() {
        return Err("工作目录必须是文件夹".to_string());
    }
    Ok(path)
}

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
    find_source_codex_home()
}

fn find_source_codex_home() -> PathBuf {
    let configured = crate::codex_config::get_codex_config_dir();
    if configured.join("config.toml").is_file() || configured.join("auth.json").is_file() {
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

fn build_exec_command(session: &CodexExecSession, codex_home: &Path) -> Result<Command, String> {
    let executable = super::misc::resolve_tool_executable_for_gui("codex");
    let mut command = codex_assistant_command_for(&executable)?;
    command.env("CODEX_HOME", codex_home);
    command.env("NO_COLOR", "1");
    // Suppress verbose telemetry (codex_otel.log_only) that floods the
    // frontend stderr channel when the dev server inherits RUST_LOG=info.
    command.env("RUST_LOG", "warn");

    let thread_id = session
        .thread_id
        .lock()
        .map_err(|_| "Codex thread 状态锁不可用".to_string())?
        .clone();

    if let Some(tid) = thread_id {
        command
            .arg("exec")
            .arg("resume")
            .arg(&tid)
            .arg("--json")
            .arg("--dangerously-bypass-approvals-and-sandbox")
            .arg("--skip-git-repo-check")
            .arg("-");
    } else {
        command
            .arg("exec")
            .arg("--json")
            .arg("-s")
            .arg("danger-full-access")
            .arg("-c")
            .arg("approval_policy=\"never\"")
            .arg("--skip-git-repo-check")
            .arg("-C")
            .arg(&session.cwd)
            .arg("-");
    }

    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(&session.cwd);

    Ok(command)
}
fn handle_exec_event(session: &CodexExecSession, json: &Value) {
    let event_type = json.get("type").and_then(Value::as_str).unwrap_or("");
    match event_type {
        "thread.started" => {
            if let Some(tid) = json.get("thread_id").and_then(Value::as_str) {
                if let Ok(mut guard) = session.thread_id.lock() {
                    *guard = Some(tid.to_string());
                }
                log::info!("[codex-assistant] thread_id={}", tid);
            }
        }
        "turn.started" => {
            emit_event(
                session.events.as_ref(),
                CodexAssistantEvent::Started {
                    session_id: session.id.clone(),
                },
            );
        }
        "item.completed" => {
            if let Some(item) = json.get("item") {
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                match item_type {
                    "agent_message" => {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            emit_event(
                                session.events.as_ref(),
                                CodexAssistantEvent::Message {
                                    session_id: session.id.clone(),
                                    message: text.to_string(),
                                },
                            );
                        }
                    }
                    "error" => {
                        let msg = item
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Codex 报告错误");
                        emit_event(
                            session.events.as_ref(),
                            CodexAssistantEvent::Log {
                                session_id: session.id.clone(),
                                message: format!("[error] {msg}"),
                            },
                        );
                    }
                    "reasoning" => {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            emit_event(
                                session.events.as_ref(),
                                CodexAssistantEvent::Log {
                                    session_id: session.id.clone(),
                                    message: text.to_string(),
                                },
                            );
                        }
                    }
                    _ => {
                        if let Some(summary) = summarize_item(item) {
                            emit_event(
                                session.events.as_ref(),
                                CodexAssistantEvent::Log {
                                    session_id: session.id.clone(),
                                    message: summary,
                                },
                            );
                        }
                    }
                }
            }
        }
        "turn.completed" => {
            session.turn_completed.store(true, Ordering::Release);
            // Release the running lock immediately so the next message can be
            // accepted without waiting for the process to exit.
            session.running.store(false, Ordering::Release);
            emit_event(
                session.events.as_ref(),
                CodexAssistantEvent::Finished {
                    session_id: session.id.clone(),
                    success: true,
                    cancelled: false,
                    message: None,
                },
            );
        }
        "turn.failed" => {
            session.turn_completed.store(true, Ordering::Release);
            session.running.store(false, Ordering::Release);
            let error = json
                .pointer("/error/message")
                .or_else(|| json.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("Codex 回合失败");
            emit_event(
                session.events.as_ref(),
                CodexAssistantEvent::Finished {
                    session_id: session.id.clone(),
                    success: false,
                    cancelled: false,
                    message: Some(error.to_string()),
                },
            );
        }
        _ => {}
    }
}

fn summarize_item(item: &Value) -> Option<String> {
    let item_type = item.get("type").and_then(Value::as_str)?;
    match item_type {
        "command_execution" => {
            let command = match item.get("command") {
                Some(Value::Array(arr)) => arr
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            let exit_code = item.get("exitCode").or_else(|| item.get("exit_code"));
            match exit_code {
                Some(code) => Some(format!("$ {command}  [exit {code}]")),
                None => Some(format!("$ {command}")),
            }
        }
        "web_search" => {
            let query = item
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("");
            Some(format!("[web_search] {query}"))
        }
        "file_change" => {
            let path = item
                .get("path")
                .or_else(|| item.get("filePath"))
                .and_then(Value::as_str)
                .unwrap_or("?");
            let action = item
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("change");
            Some(format!("[{action}] {path}"))
        }
        "tool_use" | "mcp_tool_call" => {
            let name = item
                .get("name")
                .or_else(|| item.get("tool"))
                .and_then(Value::as_str)
                .unwrap_or("tool");
            Some(format!("[tool] {name}"))
        }
        _ => item.get("text").and_then(Value::as_str).map(str::to_string),
    }
}

fn read_stdout(session: Arc<CodexExecSession>, stdout: impl Read + Send + 'static, gen: u64) {
    log::info!("[codex-assistant] stdout reader started session={}", session.id);
    for line in BufReader::new(stdout).lines() {
        match line {
            Ok(line) if !line.trim().is_empty() => match serde_json::from_str::<Value>(&line) {
                Ok(json) => handle_exec_event(&session, &json),
                Err(_) => {
                    log::info!("[codex-assistant] non-json stdout: {}", &line);
                    emit_event(
                        session.events.as_ref(),
                        CodexAssistantEvent::Log {
                            session_id: session.id.clone(),
                            message: line,
                        },
                    );
                }
            },
            Ok(_) => {}
            Err(ref e) => {
                log::warn!("[codex-assistant] stdout read error: {e}");
                break;
            }
        }
    }

    log::info!(
        "[codex-assistant] stdout reader ended session={} gen={} completed={} cancelled={}",
        session.id,
        gen,
        session.turn_completed.load(Ordering::Acquire),
        session.cancelled.load(Ordering::Acquire)
    );

    // If a newer message has already started (generation mismatch), this
    // reader is stale and must not touch session state or emit events.
    if session.generation.load(Ordering::Acquire) != gen {
        log::info!("[codex-assistant] stale stdout reader gen={} superseded, exiting quietly", gen);
        return;
    }

    let completed = session.turn_completed.swap(false, Ordering::AcqRel);
    let cancelled = session.cancelled.swap(false, Ordering::AcqRel);
    session.running.store(false, Ordering::Release);

    if !completed {
        if cancelled {
            emit_event(
                session.events.as_ref(),
                CodexAssistantEvent::Finished {
                    session_id: session.id.clone(),
                    success: false,
                    cancelled: true,
                    message: None,
                },
            );
        } else {
            emit_event(
                session.events.as_ref(),
                CodexAssistantEvent::Disconnected {
                    session_id: session.id.clone(),
                    message: "Codex 进程意外退出，请重新发送消息重试".to_string(),
                },
            );
        }
    }
}

fn read_stderr(session: Arc<CodexExecSession>, stderr: impl Read + Send + 'static) {
    for line in BufReader::new(stderr).lines() {
        match line {
            Ok(line) if !line.trim().is_empty() => {
                emit_event(
                    session.events.as_ref(),
                    CodexAssistantEvent::Stderr {
                        session_id: session.id.clone(),
                        message: line,
                    },
                );
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

fn get_session(session_id: &str) -> Result<Arc<CodexExecSession>, String> {
    SESSIONS
        .lock()
        .map_err(|_| "Codex 会话状态锁不可用".to_string())?
        .get(session_id)
        .cloned()
        .ok_or_else(|| "Codex 会话不存在或已关闭".to_string())
}

fn terminate_child(session: &CodexExecSession) {
    let child_handle = {
        let Ok(mut guard) = session.child.lock() else {
            return;
        };
        guard.take()
    };
    if let Some(mut child) = child_handle {
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
}

#[tauri::command]
pub fn start_codex_assistant_session(app: AppHandle) -> Result<String, String> {
    log::info!("[codex-assistant] start_session called");
    let cwd = resolve_workspace_dir(None)?;
    let session = Arc::new(CodexExecSession {
        id: Uuid::new_v4().to_string(),
        events: Arc::new(TauriEventSink(app)),
        child: Mutex::new(None),
        thread_id: Mutex::new(None),
        cwd,
        running: AtomicBool::new(false),
        turn_completed: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        generation: AtomicU64::new(0),
    });
    SESSIONS
        .lock()
        .map_err(|_| "Codex 会话状态锁不可用".to_string())?
        .insert(session.id.clone(), session.clone());
    log::info!("[codex-assistant] session ready id={}", session.id);
    Ok(session.id.clone())
}

#[tauri::command]
pub fn send_codex_assistant_message(session_id: String, message: String) -> Result<(), String> {
    let input = validate_input(&message)?;
    let session = get_session(&session_id)?;

    if session
        .running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("当前 Codex 回合尚未结束".to_string());
    }

    session.turn_completed.store(false, Ordering::Release);
    session.cancelled.store(false, Ordering::Release);
    // Bump generation so the previous stdout reader knows it is stale.
    let gen = session.generation.fetch_add(1, Ordering::AcqRel) + 1;

    let codex_home = codex_assistant_home();
    let mut command = build_exec_command(&session, &codex_home)?;

    // Diagnostic: log the resolved command for debugging spawn issues
    {
        let exe = command.get_program();
        let args: Vec<_> = command.get_args().collect();
        log::info!(
            "[codex-assistant] spawn: exe={:?} args={:?} codex_home={:?} cwd={:?} thread_id={:?}",
            exe, args, codex_home, session.cwd,
            session.thread_id.lock().ok().and_then(|g| g.clone()).unwrap_or_default()
        );
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    let mut child = command.spawn().map_err(|error| {
        session.running.store(false, Ordering::Release);
        format!("无法启动 Codex exec: {error}")
    })?;

    // Immediate feedback so the user sees something instead of a blank spinner
    // during the ~5-10s cold start (Node.js + runtime + skills/plugins load).
    let is_resume = session
        .thread_id
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .is_some();
    emit_event(
        session.events.as_ref(),
        CodexAssistantEvent::Log {
            session_id: session.id.clone(),
            message: if is_resume {
                "正在恢复会话上下文…".to_string()
            } else {
                "正在启动 Codex 运行时（加载技能、插件、MCP 服务器）…".to_string()
            },
        },
    );

    if let Some(mut stdin) = child.stdin.take() {
        match stdin.write_all(input.as_bytes()) {
            Ok(()) => log::info!("[codex-assistant] stdin written {} bytes", input.len()),
            Err(error) => log::warn!("[codex-assistant] failed to write stdin: {error}"),
        }
        // explicit drop to close pipe -> sends EOF to codex
        drop(stdin);
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法读取 Codex exec 输出".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取 Codex exec 错误输出".to_string())?;

    if let Ok(mut guard) = session.child.lock() {
        *guard = Some(child);
    }

    let stdout_session = session.clone();
    thread::spawn(move || read_stdout(stdout_session, stdout, gen));
    let stderr_session = session.clone();
    thread::spawn(move || read_stderr(stderr_session, stderr));

    log::info!("[codex-assistant] exec spawned session={}", session_id);
    Ok(())
}

#[tauri::command]
pub fn cancel_codex_assistant_run(session_id: String) -> Result<bool, String> {
    let session = get_session(&session_id)?;
    session.cancelled.store(true, Ordering::Release);
    terminate_child(&session);
    // Bump generation so the stale stdout reader exits quietly without
    // emitting a duplicate Finished event.
    session.generation.fetch_add(1, Ordering::AcqRel);
    // Set running=false immediately — don't wait for the stdout reader
    // post-loop, because MCP child processes may keep the pipe open.
    session.running.store(false, Ordering::Release);
    emit_event(
        session.events.as_ref(),
        CodexAssistantEvent::Finished {
            session_id: session.id.clone(),
            success: false,
            cancelled: true,
            message: None,
        },
    );
    Ok(true)
}

#[tauri::command]
pub fn close_codex_assistant_session(session_id: String) -> Result<bool, String> {
    let session = get_session(&session_id)?;
    session.cancelled.store(true, Ordering::Release);
    terminate_child(&session);
    session.generation.fetch_add(1, Ordering::AcqRel);
    session.running.store(false, Ordering::Release);
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
        session.cancelled.store(true, Ordering::Release);
        terminate_child(&session);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    static TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[derive(Clone, Default)]
    struct RecordingEventSink(Arc<Mutex<Vec<CodexAssistantEvent>>>);

    impl CodexEventSink for RecordingEventSink {
        fn emit(&self, event: CodexAssistantEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn test_session(id: &str) -> (Arc<CodexExecSession>, RecordingEventSink) {
        let events = RecordingEventSink::default();
        let session = Arc::new(CodexExecSession {
            id: id.to_string(),
            events: Arc::new(events.clone()),
            child: Mutex::new(None),
            thread_id: Mutex::new(None),
            cwd: PathBuf::from("."),
            running: AtomicBool::new(false),
            turn_completed: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        });
        (session, events)
    }

    #[test]
    fn validates_and_bounds_turn_input() {
        assert!(validate_input("  ").is_err());
        assert_eq!(validate_input("  hello  ").unwrap(), "hello");
        assert!(validate_input(&"x".repeat(MAX_INPUT_LENGTH + 1)).is_err());
    }

    #[test]
    fn thread_started_extracts_thread_id() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (session, events) = test_session("t1");
        handle_exec_event(
            &session,
            &json!({
                "type": "thread.started",
                "thread_id": "01a0c3d2-0000-0000-0000-000000000001"
            }),
        );
        assert_eq!(
            *session.thread_id.lock().unwrap(),
            Some("01a0c3d2-0000-0000-0000-000000000001".to_string())
        );
        assert!(events.0.lock().unwrap().is_empty());
    }

    #[test]
    fn turn_started_emits_started_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (session, events) = test_session("t2");
        handle_exec_event(&session, &json!({"type": "turn.started"}));
        let events = events.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            CodexAssistantEvent::Started { session_id } if session_id == "t2"
        ));
    }

    #[test]
    fn agent_message_emits_message_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (session, events) = test_session("t3");
        handle_exec_event(
            &session,
            &json!({
                "type": "item.completed",
                "item": {
                    "id": "item_0",
                    "type": "agent_message",
                    "text": "Hello world"
                }
            }),
        );
        let events = events.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            CodexAssistantEvent::Message { session_id, message }
            if session_id == "t3" && message == "Hello world"
        ));
    }

    #[test]
    fn error_item_emits_log_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (session, events) = test_session("t4");
        handle_exec_event(
            &session,
            &json!({
                "type": "item.completed",
                "item": {
                    "id": "item_0",
                    "type": "error",
                    "message": "Something went wrong"
                }
            }),
        );
        let events = events.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            CodexAssistantEvent::Log { message, .. }
            if message.contains("Something went wrong")
        ));
    }

    #[test]
    fn turn_completed_emits_finished_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (session, events) = test_session("t5");
        handle_exec_event(&session, &json!({"type": "turn.completed"}));
        assert!(session.turn_completed.load(Ordering::Acquire));
        let events = events.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            CodexAssistantEvent::Finished {
                success: true,
                cancelled: false,
                ..
            }
        ));
    }

    #[test]
    fn turn_failed_emits_finished_with_error() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (session, events) = test_session("t6");
        handle_exec_event(
            &session,
            &json!({
                "type": "turn.failed",
                "error": { "message": "rate limited" }
            }),
        );
        assert!(session.turn_completed.load(Ordering::Acquire));
        let events = events.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            CodexAssistantEvent::Finished { success: false, cancelled: false, message, .. }
            if message.as_deref() == Some("rate limited")
        ));
    }

    #[test]
    fn command_execution_summarized_as_log() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (session, events) = test_session("t7");
        handle_exec_event(
            &session,
            &json!({
                "type": "item.completed",
                "item": {
                    "id": "item_0",
                    "type": "command_execution",
                    "command": ["ls", "-la"],
                    "exitCode": 0
                }
            }),
        );
        let events = events.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            CodexAssistantEvent::Log { message, .. }
            if message.contains("ls -la") && message.contains("exit 0")
        ));
    }

    #[test]
    fn reasoning_emits_log_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        let (session, events) = test_session("t8");
        handle_exec_event(
            &session,
            &json!({
                "type": "item.completed",
                "item": {
                    "id": "item_0",
                    "type": "reasoning",
                    "text": "thinking..."
                }
            }),
        );
        let events = events.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            CodexAssistantEvent::Log { message, .. }
            if message == "thinking..."
        ));
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

    #[test]
    fn serializes_field_names_as_camel_case() {
        let event = CodexAssistantEvent::Started {
            session_id: "test-session".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            json.contains("sessionId"),
            "expected camelCase sessionId in JSON, got: {json}"
        );
        assert!(
            !json.contains("session_id"),
            "found snake_case session_id in JSON: {json}"
        );
    }
}

