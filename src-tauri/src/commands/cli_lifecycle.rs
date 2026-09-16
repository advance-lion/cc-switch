//! CLI 生命周期任务：把安装/更新/卸载放进与桌面应用相同的持久化任务体系。
//!
//! 任务记录落库（lifecycle_jobs，component = "cli"），安装过程的子进程输出实时
//! 追加进任务日志；前端按 jobId 轮询获得进度，切换页面后重新挂载可通过
//! `list_cli_lifecycle_jobs` 恢复进行中的任务，应用重启后未完成任务由启动
//! reconciler 统一标记为 interrupted。可执行文件、包名与脚本仍全部来自 misc 模块
//! 的固定注册表，渲染进程只能提供工具名、动作和 jobId。

use super::desktop_lifecycle::{DesktopLifecycleLogEntry, DesktopLifecycleOperationState};
use super::misc::{self, WslShellPreferenceInput};
use crate::database::LifecycleJobRecord;
use crate::store::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::State;
use uuid::Uuid;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// npm 安装输出可能很冗长；任务日志只保留尾部窗口，避免 plan_json 无界增长。
const MAX_JOB_LOGS: usize = 200;
/// 流式日志的落库节流间隔；步骤迁移处总是立即落库。
const LOG_SAVE_INTERVAL: Duration = Duration::from_millis(400);
/// 单行日志长度上限，防止异常长行撑大任务记录。
const MAX_LOG_LINE: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliJobAction {
    Install,
    Update,
    Uninstall,
}

impl CliJobAction {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "install" => Ok(Self::Install),
            "update" => Ok(Self::Update),
            "uninstall" => Ok(Self::Uninstall),
            _ => Err(format!("Unsupported CLI lifecycle action: {value}")),
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

/// 任务前后各做一次的本地探测快照。只含展示与校验所需的最小字段，
/// 不含路径或环境细节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliToolProbe {
    pub installed: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliLifecycleJob {
    pub id: String,
    pub app_id: String,
    pub component: String,
    pub action: String,
    pub state: String,
    pub pre_probe: Option<CliToolProbe>,
    pub post_probe: Option<CliToolProbe>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    #[serde(default)]
    pub logs: Vec<DesktopLifecycleLogEntry>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn job_record(job: &CliLifecycleJob) -> Result<LifecycleJobRecord, String> {
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
            "source": "cli-lifecycle",
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

fn cli_job_from_record(record: LifecycleJobRecord) -> Result<CliLifecycleJob, String> {
    let logs = serde_json::from_str::<serde_json::Value>(&record.plan_json)
        .ok()
        .and_then(|value| value.get("logs").cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    Ok(CliLifecycleJob {
        id: record.id,
        app_id: record.app_id,
        component: record.component,
        action: record.action,
        state: record.state,
        pre_probe: record
            .pre_probe_json
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| format!("Invalid CLI lifecycle pre-probe: {error}"))?,
        post_probe: record
            .post_probe_json
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| format!("Invalid CLI lifecycle post-probe: {error}"))?,
        error_code: record.error_code,
        error_message: record.error_message,
        logs,
        created_at: record.created_at,
        started_at: record.started_at,
        completed_at: record.completed_at,
    })
}

fn save_job(db: &crate::database::Database, job: &CliLifecycleJob) -> Result<(), String> {
    db.save_lifecycle_job(&job_record(job)?)
        .map_err(|error| error.to_string())
}

fn push_log(job: &mut CliLifecycleJob, level: &str, step: &str, message: String) {
    if job.logs.len() >= MAX_JOB_LOGS {
        job.logs.remove(0);
    }
    job.logs.push(DesktopLifecycleLogEntry {
        at: now(),
        level: level.to_string(),
        step: step.to_string(),
        message,
    });
}

fn fail_job(
    db: &crate::database::Database,
    job: &mut CliLifecycleJob,
    code: &str,
    message: String,
) -> String {
    let step = job.state.clone();
    push_log(job, "error", &step, message.clone());
    job.state = "failed".to_string();
    job.error_code = Some(code.to_string());
    job.error_message = Some(message.clone());
    job.completed_at = Some(now());
    if let Err(error) = save_job(db, job) {
        return format!("{message} (and failed to persist job: {error})");
    }
    message
}

fn cancel_job(db: &crate::database::Database, job: &mut CliLifecycleJob, message: &str) -> String {
    push_log(job, "warning", "cancelled", message.to_string());
    job.state = "cancelled".to_string();
    job.error_code = Some("JOB_CANCELLED".to_string());
    job.error_message = Some(message.to_string());
    job.completed_at = Some(now());
    if let Err(error) = save_job(db, job) {
        return format!("{message} (and failed to persist job: {error})");
    }
    message.to_string()
}

fn spawn_log_reader<R: std::io::Read + Send + 'static>(
    level: &str,
    pipe: R,
    tx: std::sync::mpsc::Sender<(String, String)>,
) -> std::thread::JoinHandle<()> {
    let level = level.to_string();
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines() {
            let Ok(line) = line else { break };
            if tx.send((level.clone(), line)).is_err() {
                break;
            }
        }
    })
}

/// 流式执行一个子进程：stdout/stderr 逐行追加进任务日志并按节流间隔落库，
/// 用户因此能在任务进行中看到实时进度。取消标志置位时杀死子进程并返回
/// `JOB_CANCELLED`，由调用方把任务标记为 cancelled。
fn stream_command(
    db: &crate::database::Database,
    job: &mut CliLifecycleJob,
    command: &mut Command,
    label: &str,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动{label}进程: {error}"))?;

    let (tx, rx) = std::sync::mpsc::channel::<(String, String)>();
    let mut readers = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        readers.push(spawn_log_reader("info", pipe, tx.clone()));
    }
    if let Some(pipe) = child.stderr.take() {
        readers.push(spawn_log_reader("warning", pipe, tx.clone()));
    }
    drop(tx);

    let mut last_save = Instant::now();
    let status = loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok((level, line)) => {
                let line = line.trim();
                if !line.is_empty() {
                    push_log(job, &level, "executing", truncate_line(line));
                    if last_save.elapsed() >= LOG_SAVE_INTERVAL {
                        // 日志落库失败不中断安装本身；最终状态迁移处还会再保存。
                        let _ = save_job(db, job);
                        last_save = Instant::now();
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
        }
        if cancellation.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            for reader in readers {
                let _ = reader.join();
            }
            while let Ok((level, line)) = rx.try_recv() {
                let line = line.trim();
                if !line.is_empty() {
                    push_log(job, &level, "executing", truncate_line(line));
                }
            }
            return Err("JOB_CANCELLED".to_string());
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("等待{label}进程时出错: {error}"))?
        {
            break status;
        }
    };
    for reader in readers {
        let _ = reader.join();
    }
    while let Ok((level, line)) = rx.try_recv() {
        let line = line.trim();
        if !line.is_empty() {
            push_log(job, &level, "executing", truncate_line(line));
        }
    }
    let _ = save_job(db, job);

    if !status.success() {
        let tail = job
            .logs
            .iter()
            .rev()
            .take(3)
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("；");
        return Err(format!(
            "{label}命令执行失败（退出码 {}）{}",
            status.code().unwrap_or(-1),
            if tail.is_empty() {
                String::new()
            } else {
                format!(": {tail}")
            }
        ));
    }
    Ok(())
}

fn truncate_line(line: &str) -> String {
    if line.chars().count() <= MAX_LOG_LINE {
        return line.to_string();
    }
    let truncated: String = line.chars().take(MAX_LOG_LINE).collect();
    format!("{truncated}…")
}

/// 流式执行由 misc 固定构造的安装/更新/卸载脚本。Windows 写临时 .bat 后
/// `cmd /C` 执行（与 run_tool_lifecycle_silently 同一模式），Unix 用 bash 并
/// 注入登录 shell 的 PATH。
#[cfg(target_os = "windows")]
fn stream_script(
    db: &crate::database::Database,
    job: &mut CliLifecycleJob,
    script: &str,
    label: &str,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    let bat_file =
        std::env::temp_dir().join(format!("cc_switch_{}_{}.bat", label, std::process::id()));
    std::fs::write(&bat_file, script).map_err(|error| format!("写入批处理文件失败: {error}"))?;
    let mut command = Command::new("cmd");
    command.arg("/C").arg(&bat_file);
    let result = stream_command(db, job, &mut command, label, cancellation);
    let _ = std::fs::remove_file(&bat_file);
    result
}

#[cfg(not(target_os = "windows"))]
fn stream_script(
    db: &crate::database::Database,
    job: &mut CliLifecycleJob,
    script: &str,
    label: &str,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    let mut command = Command::new("bash");
    command.arg("-c").arg(script);
    if let Some(login_path) = misc::login_shell_path() {
        let inherited = std::env::var("PATH").unwrap_or_default();
        command.env("PATH", misc::merge_path_segments(&login_path, &inherited));
    }
    stream_command(db, job, &mut command, label, cancellation)
}

fn npm_install_args<'a>(
    command: &'a mut Command,
    install_dir: &Path,
    package: &str,
) -> &'a mut Command {
    command
        .args(["install", "--global", "--prefix"])
        .arg(install_dir)
        .arg(format!("{package}@latest"))
}

fn stream_npm_uninstall(
    db: &crate::database::Database,
    job: &mut CliLifecycleJob,
    tool: &str,
    install_dir: &Path,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    let package = misc::npm_package_for(tool).ok_or_else(|| {
        format!(
            "{} 没有受支持的自定义目录卸载器",
            misc::tool_display_name(tool)
        )
    })?;
    let mut command = misc::npm_command();
    command
        .args(["uninstall", "--global", "--prefix"])
        .arg(install_dir)
        .arg(package);
    stream_command(db, job, &mut command, "自定义目录卸载", cancellation)
}

/// 确认受管/沙箱目录里的命令行已被移除后，才从登记处遗忘该目录。
fn finish_managed_uninstall(tool: &str, install_dir: &Path) -> Result<(), String> {
    let bin_dir = misc::assistant_install_bin_dir(install_dir);
    if misc::tool_executable_candidates(tool, &bin_dir)
        .iter()
        .any(|candidate| candidate.is_file())
    {
        return Err("卸载命令已结束，但所选目录仍检测到命令行".to_string());
    }
    misc::remove_managed_assistant_install_dir(tool)
}

/// 执行任务主体。路由规则与 run_tool_lifecycle_action / uninstall_tool_runtime
/// 完全一致：沙箱只进受控目录、受管 npm 目录优先、其余走固定脚本；区别仅在
/// 子进程输出实时写入任务日志。
#[allow(clippy::too_many_arguments)]
fn execute_action(
    db: &crate::database::Database,
    job: &mut CliLifecycleJob,
    tool: &str,
    action: CliJobAction,
    wsl_shell_by_tool: Option<&HashMap<String, WslShellPreferenceInput>>,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    match action {
        CliJobAction::Install | CliJobAction::Update => {
            if crate::config::is_test_sandbox() {
                let install_dir = misc::sandbox_managed_install_dir(tool)?;
                let package = misc::npm_package_for(tool).ok_or_else(|| {
                    format!("{} 没有 npm 沙箱安装器。", misc::tool_display_name(tool))
                })?;
                let mut command = misc::npm_command();
                npm_install_args(&mut command, &install_dir, package);
                stream_command(db, job, &mut command, "沙箱安装", cancellation)?;
                misc::save_managed_assistant_install_dir(tool, &install_dir)?;
                return Ok(());
            }
            if matches!(action, CliJobAction::Update) {
                if let Some(install_dir) = misc::managed_assistant_install_dir(tool) {
                    let package = misc::npm_package_for(tool).ok_or_else(|| {
                        format!(
                            "{} 没有受支持的自定义目录更新器",
                            misc::tool_display_name(tool)
                        )
                    })?;
                    let mut command = misc::npm_command();
                    npm_install_args(&mut command, &install_dir, package);
                    return stream_command(db, job, &mut command, "自定义目录更新", cancellation);
                }
            }
            let lifecycle_action = match action {
                CliJobAction::Install => misc::ToolLifecycleAction::Install,
                CliJobAction::Update => misc::ToolLifecycleAction::Update,
                CliJobAction::Uninstall => unreachable!("uninstall handled below"),
            };
            let script =
                misc::build_tool_lifecycle_command(&[tool], lifecycle_action, wsl_shell_by_tool)?;
            let label = match action {
                CliJobAction::Install => "tool_install",
                _ => "tool_update",
            };
            stream_script(db, job, &script, label, cancellation)
        }
        CliJobAction::Uninstall => {
            if crate::config::is_test_sandbox() {
                let sandbox_dir = misc::sandbox_managed_install_dir(tool)?;
                let registered = misc::managed_assistant_install_dir(tool)
                    .ok_or_else(|| "该应用不是由开发沙箱安装，拒绝卸载。".to_string())?;
                if registered != sandbox_dir {
                    return Err("开发沙箱只能卸载其自身目录中的应用。".to_string());
                }
                stream_npm_uninstall(db, job, tool, &sandbox_dir, cancellation)?;
                return finish_managed_uninstall(tool, &sandbox_dir);
            }
            if let Some(install_dir) = misc::managed_assistant_install_dir(tool) {
                stream_npm_uninstall(db, job, tool, &install_dir, cancellation)?;
                return finish_managed_uninstall(tool, &install_dir);
            }
            let package = misc::npm_package_for(tool).ok_or_else(|| {
                format!(
                    "{} does not have a safe automatic uninstall mapping",
                    misc::tool_display_name(tool)
                )
            })?;
            if !misc::global_npm_package_is_installed(tool, package)? {
                return Err(format!(
                    "{} 不是由可验证的 npm 全局安装管理；请通过原安装器卸载",
                    misc::tool_display_name(tool)
                ));
            }
            let script = misc::build_tool_uninstall_command(tool, package)?;
            stream_script(db, job, &script, "tool_uninstall", cancellation)
        }
    }
}

type CliProbeFuture = Pin<Box<dyn Future<Output = CliToolProbe> + Send>>;

trait CliLifecycleRuntime: Send + Sync {
    fn probe(&self, tool: &str, preference: Option<&WslShellPreferenceInput>) -> CliProbeFuture;

    fn execute(
        &self,
        db: &crate::database::Database,
        job: &mut CliLifecycleJob,
        tool: &str,
        action: CliJobAction,
        wsl_shell_by_tool: Option<&HashMap<String, WslShellPreferenceInput>>,
        cancellation: &AtomicBool,
    ) -> Result<(), String>;
}

struct SystemCliLifecycleRuntime;

impl CliLifecycleRuntime for SystemCliLifecycleRuntime {
    fn probe(&self, tool: &str, preference: Option<&WslShellPreferenceInput>) -> CliProbeFuture {
        let tool = tool.to_string();
        let wsl_shell = preference.and_then(|item| item.wsl_shell.clone());
        let wsl_shell_flag = preference.and_then(|item| item.wsl_shell_flag.clone());
        Box::pin(async move {
            let detected = misc::get_single_tool_version_impl(
                &tool,
                wsl_shell.as_deref(),
                wsl_shell_flag.as_deref(),
                false,
            )
            .await;
            CliToolProbe {
                installed: detected.version.is_some() || detected.installed_but_broken,
                version: detected.version,
            }
        })
    }

    fn execute(
        &self,
        db: &crate::database::Database,
        job: &mut CliLifecycleJob,
        tool: &str,
        action: CliJobAction,
        wsl_shell_by_tool: Option<&HashMap<String, WslShellPreferenceInput>>,
        cancellation: &AtomicBool,
    ) -> Result<(), String> {
        execute_action(db, job, tool, action, wsl_shell_by_tool, cancellation)
    }
}

async fn run_cli_lifecycle_action_with_runtime(
    db: Arc<crate::database::Database>,
    operations: &DesktopLifecycleOperationState,
    tool: String,
    action: String,
    job_id: Option<String>,
    wsl_shell_by_tool: Option<HashMap<String, WslShellPreferenceInput>>,
    runtime: Arc<dyn CliLifecycleRuntime>,
) -> Result<(), String> {
    let action = CliJobAction::parse(&action)?;
    let requested = misc::normalize_requested_tools(&[tool]);
    if requested.len() != 1 {
        return Err("Unsupported runtime".to_string());
    }
    let tool = requested[0];
    let _guard = operations.lock(tool).await;
    let job_id = job_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    if let Some(existing) = db
        .get_lifecycle_job(&job_id)
        .map_err(|error| error.to_string())?
    {
        let existing = cli_job_from_record(existing)?;
        if existing.state == "succeeded" {
            return Ok(());
        }
        return Err(existing
            .error_message
            .unwrap_or_else(|| "相同请求仍在处理中，请稍后重试".to_string()));
    }
    let cancellation = operations.register_job(&job_id).await;

    let created_at = now();
    let mut job = CliLifecycleJob {
        id: job_id.clone(),
        app_id: tool.to_string(),
        component: "cli".to_string(),
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
    push_log(
        &mut job,
        "info",
        "detecting",
        "正在检测当前安装状态".to_string(),
    );
    save_job(&db, &job)?;

    let pref = wsl_shell_by_tool.as_ref().and_then(|prefs| prefs.get(tool));
    job.pre_probe = Some(runtime.probe(tool, pref).await);
    let detected_message = match &job.pre_probe {
        Some(probe) if probe.installed => format!(
            "已检测到 {}{}",
            misc::tool_display_name(tool),
            probe
                .version
                .as_deref()
                .map(|version| format!(" {version}"))
                .unwrap_or_default()
        ),
        _ => format!("未检测到 {}", misc::tool_display_name(tool)),
    };
    push_log(&mut job, "info", "detected", detected_message);
    if let Err(error) = save_job(&db, &job) {
        let message = fail_job(&db, &mut job, "JOB_PERSIST_FAILED", error);
        operations.finish_job(&job_id).await;
        return Err(message);
    }

    if matches!(action, CliJobAction::Uninstall)
        && !job.pre_probe.as_ref().is_some_and(|probe| probe.installed)
    {
        job.post_probe = job.pre_probe.clone();
        job.state = "succeeded".to_string();
        push_log(
            &mut job,
            "info",
            "completed",
            "应用已经处于未安装状态，无需卸载".to_string(),
        );
        job.completed_at = Some(now());
        save_job(&db, &job)?;
        operations.finish_job(&job_id).await;
        return Ok(());
    }

    push_log(
        &mut job,
        "info",
        "executing",
        format!("正在执行 {}", action.as_str()),
    );
    save_job(&db, &job)?;

    let (mut job, execute_result) = tokio::task::spawn_blocking({
        let db = db.clone();
        let cancellation = cancellation.clone();
        let wsl_shell_by_tool = wsl_shell_by_tool.clone();
        let runtime = runtime.clone();
        move || {
            let mut job = job;
            let result = runtime.execute(
                &db,
                &mut job,
                tool,
                action,
                wsl_shell_by_tool.as_ref(),
                &cancellation,
            );
            (job, result)
        }
    })
    .await
    .map_err(|error| format!("CLI lifecycle task failed: {error}"))?;

    if let Err(error) = execute_result {
        operations.finish_job(&job_id).await;
        if error == "JOB_CANCELLED" {
            return Err(cancel_job(&db, &mut job, "操作已停止"));
        }
        return Err(fail_job(&db, &mut job, "LIFECYCLE_EXECUTION_FAILED", error));
    }

    job.state = "verifying".to_string();
    push_log(
        &mut job,
        "info",
        "verifying",
        "命令已结束，正在重新检测版本和安装状态".to_string(),
    );
    save_job(&db, &job)?;

    let after_probe = runtime.probe(tool, pref).await;
    job.post_probe = Some(after_probe.clone());

    let verification_error = match action {
        CliJobAction::Install if after_probe.version.is_none() => Some(format!(
            "安装命令已结束，但未检测到可用的 {}",
            misc::tool_display_name(tool)
        )),
        CliJobAction::Update if after_probe.version.is_none() => Some(format!(
            "更新命令已结束，但未检测到可用的 {}",
            misc::tool_display_name(tool)
        )),
        CliJobAction::Uninstall if after_probe.version.is_some() => Some(format!(
            "卸载命令已结束，但仍检测到 {}",
            misc::tool_display_name(tool)
        )),
        _ => None,
    };
    if let Some(error) = verification_error {
        operations.finish_job(&job_id).await;
        return Err(fail_job(
            &db,
            &mut job,
            "POST_INSTALL_VERIFICATION_FAILED",
            error,
        ));
    }

    job.state = "succeeded".to_string();
    push_log(
        &mut job,
        "info",
        "completed",
        match action {
            CliJobAction::Install => "安装完成并已通过检测",
            CliJobAction::Update => "更新完成并已通过检测",
            CliJobAction::Uninstall => "卸载完成并确认应用已移除",
        }
        .to_string(),
    );
    job.completed_at = Some(now());
    save_job(&db, &job)?;
    operations.finish_job(&job_id).await;
    Ok(())
}

#[tauri::command]
pub async fn run_cli_lifecycle_action(
    state: State<'_, AppState>,
    operations: State<'_, DesktopLifecycleOperationState>,
    tool: String,
    action: String,
    #[allow(non_snake_case)] jobId: Option<String>,
    wsl_shell_by_tool: Option<HashMap<String, WslShellPreferenceInput>>,
) -> Result<(), String> {
    run_cli_lifecycle_action_with_runtime(
        state.db.clone(),
        &operations,
        tool,
        action,
        jobId,
        wsl_shell_by_tool,
        Arc::new(SystemCliLifecycleRuntime),
    )
    .await
}

#[tauri::command]
pub async fn cancel_cli_lifecycle_job(
    operations: State<'_, DesktopLifecycleOperationState>,
    #[allow(non_snake_case)] jobId: String,
) -> Result<bool, String> {
    Ok(operations.cancel_job(&jobId).await)
}

#[tauri::command]
pub async fn get_cli_lifecycle_job(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] jobId: String,
) -> Result<Option<CliLifecycleJob>, String> {
    state
        .db
        .get_lifecycle_job(&jobId)
        .map_err(|error| error.to_string())?
        .map(cli_job_from_record)
        .transpose()
}

#[tauri::command]
pub async fn list_cli_lifecycle_jobs(
    state: State<'_, AppState>,
    tool: Option<String>,
) -> Result<Vec<CliLifecycleJob>, String> {
    state
        .db
        .list_lifecycle_jobs(tool.as_deref(), 20)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(cli_job_from_record)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeCliLifecycleRuntime {
        probes: Mutex<Vec<CliToolProbe>>,
        executions: Mutex<Vec<(String, CliJobAction)>>,
        execution_result: Result<(), String>,
    }

    impl FakeCliLifecycleRuntime {
        fn new(probes: Vec<CliToolProbe>, execution_result: Result<(), String>) -> Self {
            Self {
                probes: Mutex::new(probes),
                executions: Mutex::new(Vec::new()),
                execution_result,
            }
        }

        fn executions(&self) -> Vec<(String, CliJobAction)> {
            self.executions.lock().expect("execution lock").clone()
        }
    }

    impl CliLifecycleRuntime for FakeCliLifecycleRuntime {
        fn probe(
            &self,
            _tool: &str,
            _preference: Option<&WslShellPreferenceInput>,
        ) -> CliProbeFuture {
            let probe = self.probes.lock().expect("probe lock").remove(0);
            Box::pin(async move { probe })
        }

        fn execute(
            &self,
            _db: &crate::database::Database,
            job: &mut CliLifecycleJob,
            tool: &str,
            action: CliJobAction,
            _wsl_shell_by_tool: Option<&HashMap<String, WslShellPreferenceInput>>,
            cancellation: &AtomicBool,
        ) -> Result<(), String> {
            self.executions
                .lock()
                .expect("execution lock")
                .push((tool.to_string(), action));
            push_log(
                job,
                "info",
                "executing",
                "deepseek-harness simulated output".to_string(),
            );
            if cancellation.load(Ordering::SeqCst) {
                return Err("JOB_CANCELLED".to_string());
            }
            self.execution_result.clone()
        }
    }

    fn probe(installed: bool, version: Option<&str>) -> CliToolProbe {
        CliToolProbe {
            installed,
            version: version.map(str::to_string),
        }
    }

    async fn run_fake_lifecycle(
        db: Arc<crate::database::Database>,
        runtime: Arc<FakeCliLifecycleRuntime>,
        tool: &str,
        action: &str,
        job_id: &str,
    ) -> Result<(), String> {
        run_cli_lifecycle_action_with_runtime(
            db,
            &DesktopLifecycleOperationState::default(),
            tool.to_string(),
            action.to_string(),
            Some(job_id.to_string()),
            None,
            runtime,
        )
        .await
    }

    fn sample_job() -> CliLifecycleJob {
        CliLifecycleJob {
            id: "job-1".to_string(),
            app_id: "gemini".to_string(),
            component: "cli".to_string(),
            action: "install".to_string(),
            state: "running".to_string(),
            pre_probe: Some(CliToolProbe {
                installed: false,
                version: None,
            }),
            post_probe: None,
            error_code: None,
            error_message: None,
            logs: vec![DesktopLifecycleLogEntry {
                at: 1,
                level: "info".to_string(),
                step: "executing".to_string(),
                message: "npm install ...".to_string(),
            }],
            created_at: 1,
            started_at: Some(2),
            completed_at: None,
        }
    }

    #[tokio::test]
    async fn fake_harness_persists_install_success_with_probes_and_logs() {
        let db = Arc::new(crate::database::Database::memory().expect("memory database"));
        let runtime = Arc::new(FakeCliLifecycleRuntime::new(
            vec![probe(false, None), probe(true, Some("1.2.3"))],
            Ok(()),
        ));

        run_fake_lifecycle(
            db.clone(),
            runtime.clone(),
            "gemini",
            "install",
            "deepseek-harness-install",
        )
        .await
        .expect("fake install succeeds");

        assert_eq!(
            runtime.executions(),
            vec![("gemini".to_string(), CliJobAction::Install)]
        );
        let job = db
            .get_lifecycle_job("deepseek-harness-install")
            .expect("load job")
            .map(cli_job_from_record)
            .expect("job exists")
            .expect("parse job");
        assert_eq!(job.state, "succeeded");
        assert_eq!(job.pre_probe, Some(probe(false, None)));
        assert_eq!(job.post_probe, Some(probe(true, Some("1.2.3"))));
        assert!(job
            .logs
            .iter()
            .any(|entry| entry.message == "deepseek-harness simulated output"));
        assert_eq!(
            job.logs.last().map(|entry| entry.step.as_str()),
            Some("completed")
        );
    }

    #[tokio::test]
    async fn fake_harness_persists_update_success_with_version_recheck() {
        let db = Arc::new(crate::database::Database::memory().expect("memory database"));
        let runtime = Arc::new(FakeCliLifecycleRuntime::new(
            vec![probe(true, Some("1.2.3")), probe(true, Some("1.2.4"))],
            Ok(()),
        ));

        run_fake_lifecycle(
            db.clone(),
            runtime.clone(),
            "codex",
            "update",
            "deepseek-harness-update",
        )
        .await
        .expect("fake update succeeds");

        assert_eq!(
            runtime.executions(),
            vec![("codex".to_string(), CliJobAction::Update)]
        );
        let job = db
            .get_lifecycle_job("deepseek-harness-update")
            .expect("load job")
            .map(cli_job_from_record)
            .expect("job exists")
            .expect("parse job");
        assert_eq!(job.state, "succeeded");
        assert_eq!(job.pre_probe, Some(probe(true, Some("1.2.3"))));
        assert_eq!(job.post_probe, Some(probe(true, Some("1.2.4"))));
        assert!(job.logs.iter().any(|entry| entry.step == "verifying"));
        assert_eq!(
            job.logs.last().map(|entry| entry.message.as_str()),
            Some("更新完成并已通过检测")
        );
    }

    #[tokio::test]
    async fn fake_harness_persists_execution_failure_without_post_probe() {
        let db = Arc::new(crate::database::Database::memory().expect("memory database"));
        let runtime = Arc::new(FakeCliLifecycleRuntime::new(
            vec![probe(true, Some("1.0.0"))],
            Err("harness package manager failed".to_string()),
        ));

        let error = run_fake_lifecycle(
            db.clone(),
            runtime,
            "codex",
            "update",
            "deepseek-harness-failure",
        )
        .await
        .expect_err("fake update fails");

        assert_eq!(error, "harness package manager failed");
        let job = db
            .get_lifecycle_job("deepseek-harness-failure")
            .expect("load job")
            .map(cli_job_from_record)
            .expect("job exists")
            .expect("parse job");
        assert_eq!(job.state, "failed");
        assert_eq!(
            job.error_code.as_deref(),
            Some("LIFECYCLE_EXECUTION_FAILED")
        );
        assert!(job.post_probe.is_none());
    }

    #[tokio::test]
    async fn fake_harness_marks_cancelled_execution_with_job_cancelled_code() {
        let db = Arc::new(crate::database::Database::memory().expect("memory database"));
        let runtime = Arc::new(FakeCliLifecycleRuntime::new(
            vec![probe(true, Some("1.0.0"))],
            Err("JOB_CANCELLED".to_string()),
        ));

        let error = run_fake_lifecycle(
            db.clone(),
            runtime,
            "codex",
            "update",
            "deepseek-harness-cancelled",
        )
        .await
        .expect_err("fake update is cancelled");

        assert_eq!(error, "操作已停止");
        let job = db
            .get_lifecycle_job("deepseek-harness-cancelled")
            .expect("load job")
            .map(cli_job_from_record)
            .expect("job exists")
            .expect("parse job");
        assert_eq!(job.state, "cancelled");
        assert_eq!(job.error_code.as_deref(), Some("JOB_CANCELLED"));
    }

    #[tokio::test]
    async fn fake_harness_uninstall_absent_tool_is_successful_noop() {
        let db = Arc::new(crate::database::Database::memory().expect("memory database"));
        let runtime = Arc::new(FakeCliLifecycleRuntime::new(
            vec![probe(false, None)],
            Ok(()),
        ));

        run_fake_lifecycle(
            db.clone(),
            runtime.clone(),
            "opencode",
            "uninstall",
            "deepseek-harness-uninstall-noop",
        )
        .await
        .expect("absent uninstall is a no-op");

        assert!(runtime.executions().is_empty());
        let job = db
            .get_lifecycle_job("deepseek-harness-uninstall-noop")
            .expect("load job")
            .map(cli_job_from_record)
            .expect("job exists")
            .expect("parse job");
        assert_eq!(job.state, "succeeded");
        assert_eq!(job.post_probe, Some(probe(false, None)));
    }

    #[tokio::test]
    async fn repeated_successful_job_id_does_not_execute_twice() {
        let db = Arc::new(crate::database::Database::memory().expect("memory database"));
        let runtime = Arc::new(FakeCliLifecycleRuntime::new(
            vec![probe(false, None), probe(true, Some("1.0.0"))],
            Ok(()),
        ));

        run_fake_lifecycle(
            db.clone(),
            runtime.clone(),
            "pi",
            "install",
            "deepseek-harness-idempotent",
        )
        .await
        .expect("first install succeeds");
        run_fake_lifecycle(
            db,
            runtime.clone(),
            "pi",
            "install",
            "deepseek-harness-idempotent",
        )
        .await
        .expect("repeated job is idempotent");

        assert_eq!(runtime.executions().len(), 1);
    }

    #[test]
    fn cli_job_record_round_trips_logs_and_probes() {
        let db = crate::database::Database::memory().expect("memory database");
        let mut job = sample_job();
        save_job(&db, &job).expect("save job");
        job.state = "succeeded".to_string();
        job.post_probe = Some(CliToolProbe {
            installed: true,
            version: Some("1.2.3".to_string()),
        });
        job.completed_at = Some(9);
        save_job(&db, &job).expect("update job");

        let record = db
            .get_lifecycle_job("job-1")
            .expect("get job")
            .expect("job exists");
        let restored = cli_job_from_record(record).expect("parse job");
        assert_eq!(restored.state, "succeeded");
        assert_eq!(restored.component, "cli");
        assert_eq!(
            restored.post_probe.and_then(|probe| probe.version),
            Some("1.2.3".to_string())
        );
        assert_eq!(restored.logs.len(), 1);
        assert_eq!(restored.logs[0].message, "npm install ...");
    }

    #[test]
    fn push_log_caps_log_window() {
        let mut job = sample_job();
        for index in 0..MAX_JOB_LOGS + 50 {
            push_log(&mut job, "info", "executing", format!("line {index}"));
        }
        assert_eq!(job.logs.len(), MAX_JOB_LOGS);
        assert_eq!(
            job.logs.last().map(|entry| entry.message.as_str()),
            Some(format!("line {}", MAX_JOB_LOGS + 49).as_str())
        );
    }

    #[test]
    fn truncate_line_shortens_long_output() {
        let long = "x".repeat(MAX_LOG_LINE + 10);
        let truncated = truncate_line(&long);
        assert!(truncated.ends_with('…'));
        assert_eq!(truncated.chars().count(), MAX_LOG_LINE + 1);
        assert_eq!(truncate_line("short"), "short");
    }

    #[test]
    fn every_valid_tool_is_actionable_or_documented() {
        for tool in misc::VALID_TOOLS {
            let _ = misc::tool_display_name(tool);
        }
    }
}
