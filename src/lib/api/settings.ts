import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Settings,
  WebDavSyncSettings,
  S3SyncSettings,
  RemoteSnapshotInfo,
} from "@/types";
import type { AppId } from "./types";

export interface ConfigTransferResult {
  success: boolean;
  message: string;
  filePath?: string;
  backupId?: string;
}

export interface WebDavTestResult {
  success: boolean;
  message?: string;
}

export interface CodexUnifyHistoryRestoreResult {
  restoredJsonlFiles: number;
  restoredStateRows: number;
  /** 还原被跳过的原因（如当前目录没有账本）；存在时不应报成功 */
  skippedReason?: string;
}

export interface WebDavSyncResult {
  status: string;
}

export type ProviderLiveMembership =
  | {
      status: "available";
      providerIds: string[];
      error?: never;
    }
  | {
      status: "unavailable";
      providerIds?: never;
      error?: string;
    };

export interface CodexAssistantChatTurn {
  role: "user" | "assistant";
  content: string;
}

export type CodexAssistantEvent =
  | { sessionId: string; kind: "started" }
  | { sessionId: string; kind: "message"; message: string }
  | { sessionId: string; kind: "log"; message: string }
  | { sessionId: string; kind: "stderr"; message: string }
  | { sessionId: string; kind: "disconnected"; message: string }
  | {
      sessionId: string;
      kind: "finished";
      success: boolean;
      cancelled: boolean;
      message?: string | null;
    };

export interface CodexDesktopStatus {
  installed: boolean;
  version: string | null;
  path: string | null;
}

export type DesktopAppId =
  | "codex-desktop"
  | "claude-desktop"
  | "hermes-desktop";
export type DesktopLifecycleAction = "install" | "update" | "uninstall";

export interface DesktopAppStatus {
  id: DesktopAppId;
  display_name: string;
  installed: boolean;
  version: string | null;
  latest_version: string | null;
  path: string | null;
  launch_target: string | null;
  package_identity: string | null;
  installation_source:
    | "microsoft_store"
    | "official_appx"
    | "official_exe"
    | "application_bundle"
    | "not_installed"
    | "unsupported_platform"
    | string;
  can_install: boolean;
  can_update: boolean;
  can_uninstall: boolean;
  can_launch: boolean;
  reason: string | null;
  installations?: Array<{
    version: string;
    path: string;
    launch_target: string | null;
    package_identity: string | null;
    installation_source: string;
  }>;
}

export interface DesktopLifecycleJob {
  id: string;
  appId: DesktopAppId;
  component: "desktop";
  action: DesktopLifecycleAction;
  state:
    | "queued"
    | "running"
    | "verifying"
    | "succeeded"
    | "failed"
    | "cancelled"
    | "interrupted";
  preProbe: DesktopAppStatus | null;
  postProbe: DesktopAppStatus | null;
  errorCode: string | null;
  errorMessage: string | null;
  logs: Array<{
    at: number;
    level: "info" | "warning" | "error" | string;
    step: string;
    message: string;
  }>;
  createdAt: number;
  startedAt: number | null;
  completedAt: number | null;
}

export type LifecycleJobState = DesktopLifecycleJob["state"];
export type LifecycleJobLogEntry = DesktopLifecycleJob["logs"][number];

export interface CliLifecycleJob {
  id: string;
  appId: string;
  component: "cli";
  action: "install" | "update" | "uninstall";
  state: LifecycleJobState;
  preProbe: { installed: boolean; version: string | null } | null;
  postProbe: { installed: boolean; version: string | null } | null;
  errorCode: string | null;
  errorMessage: string | null;
  logs: LifecycleJobLogEntry[];
  createdAt: number;
  startedAt: number | null;
  completedAt: number | null;
}

/**
 * 后端按实际安装来源给出的操作能力。前端不得从“已检测到命令行”推断可以卸载：
 * 官方安装器、商店和手动安装的文件均可能出现在 PATH 中。
 */
export interface ToolLifecycleCapabilities {
  name: string;
  can_install: boolean;
  can_update: boolean;
  can_uninstall: boolean;
  can_launch: boolean;
  installation_source: string;
  reason: string | null;
}

const WEB_ASSISTANT_BRIDGE_BASE = "/__cc-switch-dev/codex-assistant";

export const isCodexAssistantWebBridgeActive = () =>
  import.meta.env.DEV && !isTauri();

async function webAssistantRequest<T>(
  path: string,
  body?: Record<string, unknown>,
): Promise<T> {
  const response = await fetch(`${WEB_ASSISTANT_BRIDGE_BASE}${path}`, {
    method: body ? "POST" : "GET",
    headers: body ? { "Content-Type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  const payload = (await response.json()) as T & { error?: string };
  if (!response.ok)
    throw new Error(payload.error || "Web assistant bridge failed");
  return payload;
}

export const settingsApi = {
  async get(): Promise<Settings> {
    return await invoke("get_settings");
  },

  async save(settings: Settings): Promise<boolean> {
    return await invoke("save_settings", { settings });
  },

  /** 是否存在统一 Codex 会话历史的迁移备份（关闭弹窗据此显示"恢复备份"勾选） */
  async hasCodexUnifyHistoryBackup(): Promise<boolean> {
    return await invoke("has_codex_unify_history_backup");
  },

  /** 按迁移备份账本把当时迁入共享桶的官方会话还原回 openai 桶（幂等） */
  async restoreCodexUnifiedHistory(): Promise<CodexUnifyHistoryRestoreResult> {
    return await invoke("restore_codex_unified_history");
  },

  async restart(): Promise<boolean> {
    return await invoke("restart_app");
  },

  async installUpdateAndRestart(): Promise<boolean> {
    return await invoke("install_update_and_restart");
  },

  async checkUpdates(): Promise<void> {
    await invoke("check_for_updates");
  },

  async isPortable(): Promise<boolean> {
    return await invoke("is_portable_mode");
  },

  async getConfigDir(appId: AppId): Promise<string> {
    return await invoke("get_config_dir", { app: appId });
  },

  async openConfigFolder(appId: AppId): Promise<void> {
    await invoke("open_config_folder", { app: appId });
  },

  async pickDirectory(defaultPath?: string): Promise<string | null> {
    return await invoke("pick_directory", { defaultPath });
  },

  async startCodexAssistantSession(): Promise<string> {
    if (isCodexAssistantWebBridgeActive()) {
      const result = await webAssistantRequest<{ sessionId: string }>(
        "/session",
        {},
      );
      return result.sessionId;
    }
    return await invoke("start_codex_assistant_session");
  },

  async sendCodexAssistantMessage(
    sessionId: string,
    message: string,
  ): Promise<void> {
    if (isCodexAssistantWebBridgeActive()) {
      await webAssistantRequest<{ accepted: boolean }>("/message", {
        sessionId,
        message,
      });
      return;
    }
    await invoke("send_codex_assistant_message", { sessionId, message });
  },

  async cancelCodexAssistantRun(sessionId: string): Promise<boolean> {
    if (isCodexAssistantWebBridgeActive()) {
      const result = await webAssistantRequest<{ cancelled: boolean }>(
        "/cancel",
        { sessionId },
      );
      return result.cancelled;
    }
    return await invoke("cancel_codex_assistant_run", { sessionId });
  },

  async closeCodexAssistantSession(sessionId: string): Promise<boolean> {
    if (isCodexAssistantWebBridgeActive()) {
      const result = await webAssistantRequest<{ closed: boolean }>("/close", {
        sessionId,
      });
      return result.closed;
    }
    return await invoke("close_codex_assistant_session", { sessionId });
  },

  async getCodexDesktopStatus(): Promise<CodexDesktopStatus> {
    if (isCodexAssistantWebBridgeActive()) {
      return await webAssistantRequest<CodexDesktopStatus>("/desktop-status");
    }
    return await invoke("get_codex_desktop_status");
  },

  async launchCodexDesktop(): Promise<void> {
    if (isCodexAssistantWebBridgeActive()) {
      await webAssistantRequest<{ launched: boolean }>("/launch-desktop", {});
      return;
    }
    await invoke("launch_codex_desktop");
  },

  async getDesktopAppStatus(app: DesktopAppId): Promise<DesktopAppStatus> {
    if (isCodexAssistantWebBridgeActive()) {
      return await webAssistantRequest<DesktopAppStatus>(
        `/desktop-app-status?app=${encodeURIComponent(app)}`,
      );
    }
    return await invoke("get_desktop_app_status", { app });
  },

  async checkDesktopAppUpdates(app: DesktopAppId): Promise<DesktopAppStatus> {
    if (isCodexAssistantWebBridgeActive()) {
      return await webAssistantRequest<DesktopAppStatus>(
        "/desktop-app-check-updates",
        { app },
      );
    }
    return await invoke("check_desktop_app_updates", { app });
  },

  async runDesktopAppLifecycleAction(
    app: DesktopAppId,
    action: DesktopLifecycleAction,
    jobId?: string,
  ): Promise<DesktopAppStatus> {
    if (isCodexAssistantWebBridgeActive()) {
      return await webAssistantRequest<DesktopAppStatus>(
        "/desktop-app-action",
        { app, action, jobId },
      );
    }
    return await invoke("run_desktop_app_lifecycle_action", {
      app,
      action,
      jobId,
    });
  },

  async cancelDesktopLifecycleJob(jobId: string): Promise<boolean> {
    if (isCodexAssistantWebBridgeActive()) return false;
    return await invoke("cancel_desktop_lifecycle_job", { jobId });
  },

  async getDesktopLifecycleJob(
    jobId: string,
  ): Promise<DesktopLifecycleJob | null> {
    if (isCodexAssistantWebBridgeActive()) return null;
    return await invoke("get_desktop_lifecycle_job", { jobId });
  },

  async listDesktopLifecycleJobs(
    app?: DesktopAppId,
  ): Promise<DesktopLifecycleJob[]> {
    if (isCodexAssistantWebBridgeActive()) return [];
    return await invoke("list_desktop_lifecycle_jobs", { app });
  },

  async launchDesktopApp(app: DesktopAppId): Promise<void> {
    if (isCodexAssistantWebBridgeActive()) {
      await webAssistantRequest<{ launched: boolean }>("/desktop-app-launch", {
        app,
      });
      return;
    }
    await invoke("launch_desktop_app", { app });
  },

  async onCodexAssistantEvent(
    handler: (event: CodexAssistantEvent) => void,
  ): Promise<UnlistenFn> {
    if (isCodexAssistantWebBridgeActive()) {
      const stream = new EventSource(`${WEB_ASSISTANT_BRIDGE_BASE}/events`);
      stream.onmessage = (event) =>
        handler(JSON.parse(event.data) as CodexAssistantEvent);
      return () => stream.close();
    }
    return await listen("codex-assistant-event", (event) => {
      handler(event.payload as CodexAssistantEvent);
    });
  },

  async selectConfigDirectory(defaultPath?: string): Promise<string | null> {
    return await invoke("pick_directory", { defaultPath });
  },

  async getClaudeCodeConfigPath(): Promise<string> {
    return await invoke("get_claude_code_config_path");
  },

  async getAppConfigPath(): Promise<string> {
    return await invoke("get_app_config_path");
  },

  async openAppConfigFolder(): Promise<void> {
    await invoke("open_app_config_folder");
  },

  async getAppConfigDirOverride(): Promise<string | null> {
    return await invoke("get_app_config_dir_override");
  },

  async setAppConfigDirOverride(path: string | null): Promise<boolean> {
    return await invoke("set_app_config_dir_override", { path });
  },

  async applyClaudePluginConfig(options: {
    official: boolean;
  }): Promise<boolean> {
    const { official } = options;
    return await invoke("apply_claude_plugin_config", { official });
  },

  async applyClaudeOnboardingSkip(): Promise<boolean> {
    return await invoke("apply_claude_onboarding_skip");
  },

  async clearClaudeOnboardingSkip(): Promise<boolean> {
    return await invoke("clear_claude_onboarding_skip");
  },

  async saveFileDialog(defaultName: string): Promise<string | null> {
    return await invoke("save_file_dialog", { defaultName });
  },

  async openFileDialog(): Promise<string | null> {
    return await invoke("open_file_dialog");
  },

  async exportConfigToFile(filePath: string): Promise<ConfigTransferResult> {
    return await invoke("export_config_to_file", { filePath });
  },

  async importConfigFromFile(filePath: string): Promise<ConfigTransferResult> {
    return await invoke("import_config_from_file", { filePath });
  },

  // ─── WebDAV sync ──────────────────────────────────────────

  async webdavTestConnection(
    settings: WebDavSyncSettings,
    preserveEmptyPassword = true,
  ): Promise<WebDavTestResult> {
    return await invoke("webdav_test_connection", {
      settings,
      preserveEmptyPassword,
    });
  },

  async webdavSyncUpload(): Promise<WebDavSyncResult> {
    return await invoke("webdav_sync_upload");
  },

  async webdavSyncDownload(): Promise<WebDavSyncResult> {
    return await invoke("webdav_sync_download");
  },

  async webdavSyncSaveSettings(
    settings: WebDavSyncSettings,
    passwordTouched = false,
  ): Promise<{ success: boolean }> {
    return await invoke("webdav_sync_save_settings", {
      settings,
      passwordTouched,
    });
  },

  async webdavSyncFetchRemoteInfo(): Promise<
    RemoteSnapshotInfo | { empty: true }
  > {
    return await invoke("webdav_sync_fetch_remote_info");
  },

  // ===== S3 Sync API =====

  async s3TestConnection(
    settings: S3SyncSettings,
    preserveEmptyPassword = true,
  ): Promise<WebDavTestResult> {
    return await invoke("s3_test_connection", {
      settings,
      preserveEmptyPassword,
    });
  },

  async s3SyncUpload(): Promise<WebDavSyncResult> {
    return await invoke("s3_sync_upload");
  },

  async s3SyncDownload(): Promise<WebDavSyncResult> {
    return await invoke("s3_sync_download");
  },

  async s3SyncSaveSettings(
    settings: S3SyncSettings,
    passwordTouched: boolean,
  ): Promise<{ success: boolean }> {
    return await invoke("s3_sync_save_settings", {
      settings,
      passwordTouched,
    });
  },

  async s3SyncFetchRemoteInfo(): Promise<RemoteSnapshotInfo | { empty: true }> {
    return await invoke("s3_sync_fetch_remote_info");
  },

  async syncCurrentProvidersLive(): Promise<void> {
    const result = (await invoke("sync_current_providers_live")) as {
      success?: boolean;
      message?: string;
    };
    if (!result?.success) {
      throw new Error(result?.message || "Sync current providers failed");
    }
  },

  async openExternal(url: string): Promise<void> {
    try {
      const u = new URL(url);
      const scheme = u.protocol.replace(":", "").toLowerCase();
      if (scheme !== "http" && scheme !== "https") {
        throw new Error("Unsupported URL scheme");
      }
    } catch {
      throw new Error("Invalid URL");
    }
    await invoke("open_external", { url });
  },

  async setAutoLaunch(enabled: boolean): Promise<boolean> {
    return await invoke("set_auto_launch", { enabled });
  },

  async getAutoLaunchStatus(): Promise<boolean> {
    return await invoke("get_auto_launch_status");
  },

  async getToolVersions(
    tools?: string[],
    wslShellByTool?: Record<
      string,
      { wslShell?: string | null; wslShellFlag?: string | null }
    >,
  ): Promise<
    Array<{
      name: string;
      version: string | null;
      latest_version: string | null;
      error: string | null;
      installed_but_broken: boolean;
      env_type: "windows" | "wsl" | "macos" | "linux" | "unknown";
      wsl_distro: string | null;
    }>
  > {
    if (isCodexAssistantWebBridgeActive() && tools?.length === 1) {
      const tool = tools[0];
      const status =
        tool === "codex" || tool === "qoder"
          ? await webAssistantRequest<{
              available: boolean;
              version: string | null;
            }>(`/tool-status?tool=${encodeURIComponent(tool)}`)
          : { available: false, version: null };
      return [
        {
          name: tool,
          version: status.available ? status.version : null,
          latest_version: null,
          error: status.available ? null : "Codex CLI unavailable",
          installed_but_broken: false,
          env_type: "windows",
          wsl_distro: null,
        },
      ];
    }
    return await invoke("get_tool_versions", { tools, wslShellByTool });
  },

  async getToolLifecycleCapabilities(
    tools: string[],
  ): Promise<ToolLifecycleCapabilities[]> {
    // 浏览器预览没有 Tauri 后端；保持界面可演示，但不把它伪装成一次真实的
    // 安装来源验证。桌面端始终走 Rust 的受控探测。
    if (isCodexAssistantWebBridgeActive()) {
      return tools.map((name) => ({
        name,
        can_install: true,
        can_update: true,
        can_uninstall: false,
        can_launch: true,
        installation_source: "web_preview",
        reason: "浏览器预览无法验证本机安装来源。",
      }));
    }
    return await invoke("get_tool_lifecycle_capabilities", { tools });
  },

  /** 只有用户明确点“检查更新”时才访问远程版本源。 */
  async checkToolUpdates(tools: string[]): Promise<
    Array<{
      name: string;
      version: string | null;
      latest_version: string | null;
      error: string | null;
      installed_but_broken: boolean;
      env_type: "windows" | "wsl" | "macos" | "linux" | "unknown";
      wsl_distro: string | null;
    }>
  > {
    if (isCodexAssistantWebBridgeActive()) {
      return await settingsApi.getToolVersions(tools);
    }
    return await invoke("check_tool_updates", { tools });
  },

  async runToolLifecycleAction(
    tools: string[],
    action: "install" | "update",
    wslShellByTool?: Record<
      string,
      { wslShell?: string | null; wslShellFlag?: string | null }
    >,
  ): Promise<void> {
    await invoke("run_tool_lifecycle_action", {
      tools,
      action,
      wslShellByTool,
    });
  },

  /**
   * 任务化的 CLI 生命周期操作：后端把进度写入持久化任务记录，
   * 前端生成 jobId 并轮询 getCliLifecycleJob 获取实时日志；
   * 相同 jobId 重复提交幂等，不会重复执行安装脚本。
   */
  async runCliLifecycleAction(
    tool: string,
    action: "install" | "update" | "uninstall",
    jobId?: string,
    wslShellByTool?: Record<
      string,
      { wslShell?: string | null; wslShellFlag?: string | null }
    >,
  ): Promise<void> {
    await invoke("run_cli_lifecycle_action", {
      tool,
      action,
      jobId,
      wslShellByTool,
    });
  },

  async cancelCliLifecycleJob(jobId: string): Promise<boolean> {
    return await invoke("cancel_cli_lifecycle_job", { jobId });
  },

  async getCliLifecycleJob(jobId: string): Promise<CliLifecycleJob | null> {
    return await invoke("get_cli_lifecycle_job", { jobId });
  },

  async listCliLifecycleJobs(tool?: string): Promise<CliLifecycleJob[]> {
    return await invoke("list_cli_lifecycle_jobs", { tool });
  },

  /** Read authoritative live membership for an additive provider app. */
  async getProviderLiveMembership(app: AppId): Promise<ProviderLiveMembership> {
    return await invoke("get_provider_live_membership", { app });
  },

  /** Add or remove one provider from an additive app's live membership. */
  async setProviderLiveEnabled(
    app: AppId,
    providerId: string,
    enabled: boolean,
  ): Promise<ProviderLiveMembership> {
    return await invoke("set_provider_live_enabled", {
      app,
      providerId,
      enabled,
    });
  },

  /** 在用户首选终端中启动已登记 Runtime 的 CLI。后端只接受受支持的工具名。 */
  async launchToolTerminal(tool: string): Promise<void> {
    await invoke("launch_tool_terminal", { tool });
  },

  /** 启动 DeepSeek Harness Web UI（后台运行 dsh web 并打开浏览器）。 */
  async launchDsh(): Promise<void> {
    await invoke("launch_dsh");
  },

  /** 重启 DeepSeek Harness Web UI（杀掉端口 3080 上的进程后重新启动）。 */
  async restartDsh(): Promise<void> {
    await invoke("restart_dsh");
  },

  /** 卸载有明确 npm 包映射的 Runtime；不会变更 Provider 或账号配置。 */
  async uninstallToolRuntime(tool: string): Promise<void> {
    await invoke("uninstall_tool_runtime", { tool });
  },

  /** 探测各工具安装分布：枚举所有安装、标记冲突、生成锚定升级命令。
   *  诊断按钮、升级前确认、升级后补诊共用此命令，各取所需字段。 */
  async probeToolInstallations(
    tools: string[],
  ): Promise<ToolInstallationReport[]> {
    return await invoke("probe_tool_installations", { tools });
  },

  async getRectifierConfig(): Promise<RectifierConfig> {
    return await invoke("get_rectifier_config");
  },

  async setRectifierConfig(config: RectifierConfig): Promise<boolean> {
    return await invoke("set_rectifier_config", { config });
  },

  async getOptimizerConfig(): Promise<OptimizerConfig> {
    return await invoke("get_optimizer_config");
  },

  async setOptimizerConfig(config: OptimizerConfig): Promise<boolean> {
    return await invoke("set_optimizer_config", { config });
  },

  async getLogConfig(): Promise<LogConfig> {
    return await invoke("get_log_config");
  },

  async setLogConfig(config: LogConfig): Promise<boolean> {
    return await invoke("set_log_config", { config });
  },
};

/** 单处工具安装的诊断信息（多处安装冲突检测）。字段对应后端 ToolInstallation。 */
export interface ToolInstallation {
  path: string;
  version: string | null;
  runnable: boolean;
  error: string | null;
  source: string;
  is_path_default: boolean;
}

/** 一次"探测工具安装分布"的结果。字段对应后端 ToolInstallationReport。 */
export interface ToolInstallationReport {
  tool: string;
  installs: ToolInstallation[];
  is_conflict: boolean;
  needs_confirmation: boolean;
  command: string;
  anchored: boolean;
}

export interface RectifierConfig {
  enabled: boolean;
  requestThinkingSignature: boolean;
  requestThinkingBudget: boolean;
  requestMediaFallback: boolean;
  requestMediaHeuristic: boolean;
}

export interface OptimizerConfig {
  enabled: boolean;
  thinkingOptimizer: boolean;
  cacheInjection: boolean;
}

export interface LogConfig {
  enabled: boolean;
  level: "error" | "warn" | "info" | "debug" | "trace";
}

export interface BackupEntry {
  filename: string;
  sizeBytes: number;
  createdAt: string;
}

export const backupsApi = {
  async createDbBackup(): Promise<string> {
    return await invoke("create_db_backup");
  },

  async listDbBackups(): Promise<BackupEntry[]> {
    return await invoke("list_db_backups");
  },

  async restoreDbBackup(filename: string): Promise<string> {
    return await invoke("restore_db_backup", { filename });
  },

  async renameDbBackup(oldFilename: string, newName: string): Promise<string> {
    return await invoke("rename_db_backup", { oldFilename, newName });
  },

  async deleteDbBackup(filename: string): Promise<void> {
    await invoke("delete_db_backup", { filename });
  },
};
