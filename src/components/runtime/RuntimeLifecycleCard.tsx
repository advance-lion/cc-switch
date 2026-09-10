import { useCallback, useEffect, useRef, useState } from "react";
import {
  Download,
  Loader2,
  MonitorPlay,
  RefreshCw,
  Sparkles,
  Terminal,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { APP_ICON_MAP } from "@/config/appConfig";
import {
  settingsApi,
  type AppId,
  type DesktopAppId,
  type DesktopAppStatus,
  type DesktopLifecycleJob,
  type ToolLifecycleCapabilities,
} from "@/lib/api";
import { isUpdateAvailable } from "@/lib/version";
import { extractErrorMessage } from "@/utils/errorUtils";

type RuntimeTool =
  | "claude"
  | "codex"
  | "gemini"
  | "grok"
  | "opencode"
  | "openclaw"
  | "hermes"
  | "pi";

type ToolVersion = {
  name: string;
  version: string | null;
  latest_version: string | null;
  error: string | null;
  installed_but_broken: boolean;
};

const TOOL_BY_APP: Partial<Record<AppId, RuntimeTool>> = {
  claude: "claude",
  codex: "codex",
  gemini: "gemini",
  grokbuild: "grok",
  opencode: "opencode",
  openclaw: "openclaw",
  hermes: "hermes",
  pi: "pi",
};

const DESKTOP_BY_APP: Partial<Record<AppId, DesktopAppId>> = {
  claude: "claude-desktop",
  "claude-desktop": "claude-desktop",
  codex: "codex-desktop",
};

const cliCache = new Map<RuntimeTool, {
  status: ToolVersion | null;
  capabilities: ToolLifecycleCapabilities | null;
}>();
const desktopCache = new Map<DesktopAppId, DesktopAppStatus>();

interface RuntimeLifecycleCardProps {
  appId: AppId;
  /** 模型服务配置状态，与应用安装/启动严格分离。 */
  isConfigured: boolean;
  /** 安装、连接、断开等状态变化后递增，要求重新读取真实状态。 */
  refreshRequestId?: number;
  onAiInstall?: (intent: {
    tool: string;
    appName: string;
    supportsCustomLocation?: boolean;
    supportsVersionPin?: boolean;
  }) => void;
}

interface LifecycleRowProps {
  title: string;
  subtitle: string;
  installed: boolean;
  version: string | null;
  latestVersion: string | null;
  updateChecked: boolean;
  updateAvailable: boolean;
  installationSource?: string | null;
  additionalInstallationCount?: number;
  needsRepair?: boolean;
  loading: boolean;
  busy: boolean;
  action: string | null;
  canInstall: boolean;
  canUpdate: boolean;
  canUninstall: boolean;
  canLaunch: boolean;
  disabledReason?: string | null;
  launchLabel: string;
  onInstall: () => void;
  onAiInstall?: () => void;
  onLaunch: () => void;
  onCheckOrUpdate: () => void;
  onUninstall: () => void;
  onRedetect: () => void;
  onCancel?: () => void;
}

function LifecycleRow({
  title,
  subtitle,
  installed,
  version,
  latestVersion,
  updateChecked,
  updateAvailable,
  installationSource,
  additionalInstallationCount = 0,
  needsRepair = false,
  loading,
  busy,
  action,
  canInstall,
  canUpdate,
  canUninstall,
  canLaunch,
  disabledReason,
  launchLabel,
  onInstall,
  onAiInstall,
  onLaunch,
  onCheckOrUpdate,
  onUninstall,
  onRedetect,
  onCancel,
}: LifecycleRowProps) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-3 rounded-xl border bg-background/70 p-3.5 lg:flex-row lg:items-center lg:justify-between">
      <div className="min-w-0 space-y-1.5">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm font-semibold">{title}</span>
          {loading ? (
            <Badge variant="secondary">
              <Loader2 className="mr-1 h-3 w-3 animate-spin" />
              {t("appLifecycle.detecting")}
            </Badge>
          ) : installed ? (
            <Badge
              variant="outline"
              className={
                needsRepair
                  ? "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300"
                  : "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
              }
            >
              {needsRepair
                ? t("appLifecycle.needsRepair")
                : t("appLifecycle.installed", { version: version ?? "—" })}
            </Badge>
          ) : (
            <Badge variant="secondary">{t("common.notInstalled")}</Badge>
          )}
          {updateChecked && latestVersion && (
            <Badge variant="outline" className="text-muted-foreground">
              {t("appLifecycle.latestVersion", { version: latestVersion })}
            </Badge>
          )}
        </div>
        <p className="text-xs text-muted-foreground">{subtitle}</p>
        {installationSource && (
          <p className="truncate text-[11px] text-muted-foreground/80">
            {t("appLifecycle.installationSource", {
              source: installationSource,
            })}
            {additionalInstallationCount > 0 ? ` · 另检测到 ${additionalInstallationCount} 份安装` : ""}
          </p>
        )}
        {!installed && disabledReason && (
          <p className="text-[11px] text-amber-700 dark:text-amber-300">
            {disabledReason}
          </p>
        )}
      </div>

      <div className="flex shrink-0 flex-wrap items-center gap-2">
        {busy && onCancel && (
          <Button variant="outline" size="sm" onClick={onCancel}>
            停止
          </Button>
        )}
        {installed ? (
          <>
            <Button
              variant="outline"
              size="sm"
              disabled={busy || needsRepair || !canLaunch}
              onClick={onLaunch}
              title={!canLaunch ? disabledReason ?? undefined : undefined}
            >
              {action === "launch" ? (
                <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
              ) : title.includes("Desktop") ? (
                <MonitorPlay className="mr-1.5 h-3.5 w-3.5" />
              ) : (
                <Terminal className="mr-1.5 h-3.5 w-3.5" />
              )}
              {launchLabel}
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={busy || !canUpdate || (updateChecked && !updateAvailable)}
              onClick={onCheckOrUpdate}
              title={!canUpdate ? disabledReason ?? undefined : undefined}
            >
              {action === "check" || action === "update" ? (
                <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
              ) : (
                <RefreshCw className="mr-1.5 h-3.5 w-3.5" />
              )}
              {updateChecked
                ? updateAvailable
                  ? t("appLifecycle.update")
                  : t("appLifecycle.upToDate")
                : t("appLifecycle.checkUpdates")}
            </Button>
            {canUninstall && (
              <Button
                variant="outline"
                size="sm"
                className="border-destructive/30 text-destructive hover:bg-destructive/10 hover:text-destructive"
                disabled={busy}
                onClick={onUninstall}
              >
                <Trash2 className="mr-1.5 h-3.5 w-3.5" />
                {t("appLifecycle.uninstall")}
              </Button>
            )}
          </>
        ) : (
          <>
            <Button
              size="sm"
              disabled={busy || loading || !canInstall}
              onClick={onInstall}
              title={!canInstall ? disabledReason ?? undefined : undefined}
            >
              {action === "install" ? (
                <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
              ) : (
                <Download className="mr-1.5 h-3.5 w-3.5" />
              )}
              {t("appLifecycle.standardInstall")}
            </Button>
            {onAiInstall && (
              <Button
                variant="outline"
                size="sm"
                disabled={busy || loading}
                onClick={onAiInstall}
              >
                <Sparkles className="mr-1.5 h-3.5 w-3.5 text-violet-500" />
                {t("appLifecycle.aiInstall")}
              </Button>
            )}
          </>
        )}
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8"
          disabled={busy || loading}
          title={t("appLifecycle.redetect")}
          onClick={onRedetect}
        >
          <RefreshCw className="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  );
}

function CliLifecycleRow({
  tool,
  title,
  subtitle,
  refreshRequestId,
  onAiInstall,
}: {
  tool: RuntimeTool;
  title: string;
  subtitle: string;
  refreshRequestId: number;
  onAiInstall?: () => void;
}) {
  const { t } = useTranslation();
  const cached = cliCache.get(tool);
  const [status, setStatus] = useState<ToolVersion | null>(cached?.status ?? null);
  const [capabilities, setCapabilities] = useState<ToolLifecycleCapabilities | null>(
    cached?.capabilities ?? null,
  );
  const [loading, setLoading] = useState(!cached);
  const [action, setAction] = useState<string | null>(null);
  const [uninstallOpen, setUninstallOpen] = useState(false);
  const [uninstallCompletedOpen, setUninstallCompletedOpen] = useState(false);
  const lastRefreshRequestRef = useRef(refreshRequestId);

  const refresh = useCallback(async (includeLatest = false) => {
    setLoading(true);
    try {
      const [versions, lifecycleCapabilities] = await Promise.all([
        includeLatest
          ? settingsApi.checkToolUpdates([tool])
          : settingsApi.getToolVersions([tool]),
        settingsApi.getToolLifecycleCapabilities([tool]),
      ]);
      const next = versions[0] ?? null;
      const nextCapabilities = lifecycleCapabilities[0] ?? null;
      setStatus(next);
      setCapabilities(nextCapabilities);
      cliCache.set(tool, { status: next, capabilities: nextCapabilities });
      return next;
    } catch (error) {
      toast.error(t("appLifecycle.detectFailed"), {
        description: extractErrorMessage(error),
      });
      return null;
    } finally {
      setLoading(false);
    }
  }, [t, tool]);

  useEffect(() => {
    if (!cliCache.has(tool)) void refresh(false);
  }, [refresh, tool]);

  useEffect(() => {
    if (lastRefreshRequestRef.current === refreshRequestId) return;
    lastRefreshRequestRef.current = refreshRequestId;
    void refresh(false);
  }, [refresh, refreshRequestId]);

  const runLifecycle = async (nextAction: "install" | "update") => {
    setAction(nextAction);
    try {
      await settingsApi.runToolLifecycleAction([tool], nextAction);
      const detected = await refresh(false);
      if (nextAction === "install" && !detected?.version) {
        throw new Error(t("appLifecycle.installNotDetected"));
      }
      toast.success(
        t(nextAction === "install" ? "appLifecycle.installCompleted" : "appLifecycle.updateCompleted"),
      );
    } catch (error) {
      toast.error(
        t(nextAction === "install" ? "appLifecycle.installFailed" : "appLifecycle.updateFailed"),
        { description: extractErrorMessage(error) },
      );
    } finally {
      setAction(null);
    }
  };

  const launch = async () => {
    setAction("launch");
    try {
      await settingsApi.launchToolTerminal(tool);
      toast.success(t("appLifecycle.terminalOpened"));
    } catch (error) {
      toast.error(t("appLifecycle.launchFailed"), {
        description: extractErrorMessage(error),
      });
    } finally {
      setAction(null);
    }
  };

  const uninstall = async () => {
    setAction("uninstall");
    try {
      await settingsApi.uninstallToolRuntime(tool);
      const detected = await refresh(false);
      if (detected?.version) throw new Error(t("appLifecycle.uninstallStillDetected"));
      setUninstallOpen(false);
      setUninstallCompletedOpen(true);
    } catch (error) {
      toast.error(t("appLifecycle.uninstallFailed"), {
        description: extractErrorMessage(error),
      });
    } finally {
      setAction(null);
    }
  };

  const installed = Boolean(status?.version);
  const updateChecked = Boolean(status?.latest_version);
  const updateAvailable = Boolean(
    status?.version && status.latest_version && isUpdateAvailable(status.version, status.latest_version),
  );
  const busy = action !== null;

  return (
    <>
      <LifecycleRow
        title={title}
        subtitle={subtitle}
        installed={installed}
        version={status?.version ?? null}
        latestVersion={status?.latest_version ?? null}
        updateChecked={updateChecked}
        updateAvailable={updateAvailable}
        installationSource={capabilities?.installation_source}
        needsRepair={status?.installed_but_broken}
        loading={loading}
        busy={busy}
        action={action}
        canInstall={capabilities?.can_install ?? false}
        canUpdate={capabilities?.can_update ?? false}
        canUninstall={capabilities?.can_uninstall ?? false}
        canLaunch={capabilities?.can_launch ?? false}
        disabledReason={capabilities?.reason}
        launchLabel={t("appLifecycle.launch")}
        onInstall={() => void runLifecycle("install")}
        onAiInstall={onAiInstall}
        onLaunch={() => void launch()}
        onCheckOrUpdate={() =>
          void (updateChecked && updateAvailable ? runLifecycle("update") : refresh(true))
        }
        onUninstall={() => setUninstallOpen(true)}
        onRedetect={() => void refresh(false)}
      />
      <ConfirmDialog
        isOpen={uninstallOpen}
        title={t("appLifecycle.uninstallTitle", { app: title })}
        message={t("appLifecycle.uninstallMessage", { app: title })}
        confirmText={t("appLifecycle.uninstall")}
        pending={action === "uninstall"}
        onConfirm={() => void uninstall()}
        onCancel={() => setUninstallOpen(false)}
      />
      <UninstallCompletedDialog
        open={uninstallCompletedOpen}
        onOpenChange={setUninstallCompletedOpen}
        appName={title}
      />
    </>
  );
}

function DesktopLifecycleRow({
  app,
  refreshRequestId,
  onAiInstall,
}: {
  app: DesktopAppId;
  refreshRequestId: number;
  onAiInstall?: () => void;
}) {
  const { t } = useTranslation();
  const cached = desktopCache.get(app);
  const [status, setStatus] = useState<DesktopAppStatus | null>(cached ?? null);
  const [loading, setLoading] = useState(!cached);
  const [action, setAction] = useState<string | null>(null);
  const [uninstallOpen, setUninstallOpen] = useState(false);
  const [uninstallCompletedOpen, setUninstallCompletedOpen] = useState(false);
  const [uninstallCompletionMessage, setUninstallCompletionMessage] = useState<string | null>(null);
  const [lastJob, setLastJob] = useState<DesktopLifecycleJob | null>(null);
  const [activeJobId, setActiveJobId] = useState<string | null>(null);
  const [activeJob, setActiveJob] = useState<DesktopLifecycleJob | null>(null);
  const lastRefreshRequestRef = useRef(refreshRequestId);

  const refresh = useCallback(async (includeLatest = false) => {
    setLoading(true);
    try {
      const next = includeLatest
        ? await settingsApi.checkDesktopAppUpdates(app)
        : await settingsApi.getDesktopAppStatus(app);
      setStatus(next);
      desktopCache.set(app, next);
      return next;
    } catch (error) {
      toast.error(t("appLifecycle.detectFailed"), {
        description: extractErrorMessage(error),
      });
      return null;
    } finally {
      setLoading(false);
    }
  }, [app, t]);

  useEffect(() => {
    if (!desktopCache.has(app)) void refresh(false);
    void settingsApi
      .listDesktopLifecycleJobs(app)
      .then((jobs) => setLastJob(jobs[0] ?? null))
      .catch(() => undefined);
  }, [app, refresh]);

  useEffect(() => {
    if (lastRefreshRequestRef.current === refreshRequestId) return;
    lastRefreshRequestRef.current = refreshRequestId;
    void refresh(false);
  }, [refresh, refreshRequestId]);

  useEffect(() => {
    if (!activeJobId) {
      setActiveJob(null);
      return;
    }
    let disposed = false;
    const poll = async () => {
      const job = await settingsApi.getDesktopLifecycleJob(activeJobId).catch(() => null);
      if (!disposed && job) setActiveJob(job);
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 700);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [activeJobId]);

  const runLifecycle = async (nextAction: "install" | "update" | "uninstall") => {
    const jobId = crypto.randomUUID();
    setActiveJobId(jobId);
    setActiveJob(null);
    setAction(nextAction);
    try {
      const next = await settingsApi.runDesktopAppLifecycleAction(app, nextAction, jobId);
      setStatus(next);
      desktopCache.set(app, next);
      const jobs = await settingsApi.listDesktopLifecycleJobs(app).catch(() => []);
      setLastJob(jobs[0] ?? null);
      if (nextAction === "uninstall") {
        setUninstallOpen(false);
        setUninstallCompletionMessage(next.installed ? next.reason : null);
        setUninstallCompletedOpen(true);
      } else {
        toast.success(
          t(nextAction === "install" ? "appLifecycle.installCompleted" : "appLifecycle.updateCompleted"),
        );
      }
    } catch (error) {
      const jobs = await settingsApi.listDesktopLifecycleJobs(app).catch(() => []);
      setLastJob(jobs[0] ?? null);
      const key = nextAction === "install"
        ? "appLifecycle.installFailed"
        : nextAction === "update"
          ? "appLifecycle.updateFailed"
          : "appLifecycle.uninstallFailed";
      toast.error(t(key), { description: extractErrorMessage(error) });
    } finally {
      setActiveJobId(null);
      setAction(null);
    }
  };

  const cancelLifecycle = async () => {
    if (!activeJobId) return;
    const accepted = await settingsApi.cancelDesktopLifecycleJob(activeJobId);
    if (!accepted) {
      toast.error("当前操作已经结束，正在重新检测状态");
      await refresh(false);
    }
  };

  const launch = async () => {
    setAction("launch");
    try {
      await settingsApi.launchDesktopApp(app);
      toast.success(t("appLifecycle.desktopOpened", { app: status?.display_name ?? app }));
    } catch (error) {
      toast.error(t("appLifecycle.desktopLaunchFailed"), {
        description: extractErrorMessage(error),
      });
    } finally {
      setAction(null);
    }
  };

  const installed = status?.installed ?? false;
  const updateChecked = Boolean(status?.latest_version);
  const updateAvailable = Boolean(
    status?.version && status.latest_version && isUpdateAvailable(status.version, status.latest_version),
  );
  const title = status?.display_name ?? (app === "codex-desktop" ? "Codex Desktop" : "Claude Desktop");

  return (
    <>
      <div className="space-y-1.5">
        <LifecycleRow
          title={title}
          subtitle={t("appLifecycle.desktopDescription")}
          installed={installed}
          version={status?.version ?? null}
          latestVersion={status?.latest_version ?? null}
          updateChecked={updateChecked}
          updateAvailable={updateAvailable}
          installationSource={status?.installation_source}
          additionalInstallationCount={Math.max(0, (status?.installations?.length ?? 0) - 1)}
          loading={loading}
          busy={action !== null}
          action={action}
          canInstall={status?.can_install ?? false}
          canUpdate={status?.can_update ?? false}
          canUninstall={status?.can_uninstall ?? false}
          canLaunch={status?.can_launch ?? false}
          disabledReason={status?.reason}
          launchLabel={t("appLifecycle.launchDesktop")}
          onInstall={() => void runLifecycle("install")}
          onAiInstall={onAiInstall}
          onLaunch={() => void launch()}
          onCheckOrUpdate={() =>
            void (updateChecked && updateAvailable ? runLifecycle("update") : refresh(true))
          }
          onUninstall={() => setUninstallOpen(true)}
          onRedetect={() => void refresh(false)}
          onCancel={activeJobId ? () => void cancelLifecycle() : undefined}
        />
        {activeJob && ["queued", "running", "verifying"].includes(activeJob.state) && (
          <div className="rounded-lg border border-blue-500/25 bg-blue-500/5 px-3 py-2 text-xs text-foreground">
            <div className="flex items-center gap-2 font-medium">
              <Loader2 className="h-3.5 w-3.5 animate-spin text-blue-600" />
              {activeJob.state === "verifying" ? "正在重新检测安装结果" : "正在执行操作"}
            </div>
            <div className="mt-2 space-y-1 text-muted-foreground" aria-live="polite">
              {activeJob.logs.slice(-4).map((entry, index) => (
                <p key={`${entry.at}-${entry.step}-${index}`}>{entry.message}</p>
              ))}
            </div>
          </div>
        )}
        {lastJob && ["failed", "interrupted"].includes(lastJob.state) && (
          <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-xs text-amber-800 dark:text-amber-200">
            上次{lastJob.action === "install" ? "安装" : lastJob.action === "update" ? "更新" : "卸载"}
            {lastJob.state === "interrupted" ? "被中断" : "失败"}
            {lastJob.errorMessage ? `：${lastJob.errorMessage}` : "，请重新检测后重试。"}
            {lastJob.logs.length > 0 && (
              <div className="mt-2 space-y-1 border-t border-amber-500/20 pt-2 opacity-90">
                {lastJob.logs.slice(-4).map((entry, index) => (
                  <p key={`${entry.at}-${entry.step}-${index}`}>{entry.message}</p>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
      <ConfirmDialog
        isOpen={uninstallOpen}
        title={t("appLifecycle.uninstallTitle", { app: title })}
        message={t("appLifecycle.uninstallMessage", { app: title })}
        confirmText={t("appLifecycle.uninstall")}
        pending={action === "uninstall"}
        onConfirm={() => void runLifecycle("uninstall")}
        onCancel={() => setUninstallOpen(false)}
      />
      <UninstallCompletedDialog
        open={uninstallCompletedOpen}
        onOpenChange={setUninstallCompletedOpen}
        appName={title}
        message={uninstallCompletionMessage}
      />
    </>
  );
}

function UninstallCompletedDialog({
  open,
  onOpenChange,
  appName,
  message,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  appName: string;
  message?: string | null;
}) {
  const { t } = useTranslation();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm" zIndex="top">
        <DialogHeader className="space-y-3">
          <DialogTitle className="text-lg font-semibold">
            {t("appLifecycle.uninstallCompleted")}
          </DialogTitle>
          <DialogDescription className="text-sm leading-relaxed">
            {message ?? t("appLifecycle.uninstallCompletedDescription", { app: appName })}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter className="pt-2 sm:justify-end">
          <Button onClick={() => onOpenChange(false)}>{t("common.close")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * 当前应用的安装与管理入口。命令行和桌面应用是两个独立组件，版本、安装、
 * 更新、启动和卸载状态互不借用；模型服务接入状态也不会伪装成运行状态。
 */
export function RuntimeLifecycleCard({
  appId,
  isConfigured,
  refreshRequestId = 0,
  onAiInstall,
}: RuntimeLifecycleCardProps) {
  const { t } = useTranslation();
  const app = APP_ICON_MAP[appId];
  const tool = TOOL_BY_APP[appId];
  const desktop = DESKTOP_BY_APP[appId];
  const cliTitle = appId === "codex"
    ? "Codex CLI"
    : appId === "claude"
      ? "Claude Code CLI"
      : `${app.label} CLI`;

  return (
    <section className="sticky top-0 z-20 rounded-2xl border bg-background/95 p-4 shadow-sm backdrop-blur supports-[backdrop-filter]:bg-background/85">
      <div className="mb-3 flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2.5">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border bg-muted/40">
            {app.icon}
          </div>
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="truncate text-sm font-semibold">
                {t("appLifecycle.title", { app: app.label })}
              </h2>
              <Badge
                variant="outline"
                className={
                  isConfigured
                    ? "border-sky-500/30 bg-sky-500/10 text-sky-700 dark:text-sky-300"
                    : "text-muted-foreground"
                }
              >
                {isConfigured
                  ? t("appLifecycle.connected")
                  : t("appLifecycle.notConnected")}
              </Badge>
            </div>
            <p className="mt-0.5 text-xs text-muted-foreground">
              {desktop && tool
                ? t("appLifecycle.componentsIndependent")
                : t("appLifecycle.description")}
            </p>
          </div>
        </div>
      </div>

      <div className="space-y-2.5">
        {tool && (
          <CliLifecycleRow
            tool={tool}
            title={cliTitle}
            subtitle={t("appLifecycle.cliDescription")}
            refreshRequestId={refreshRequestId}
            onAiInstall={
              onAiInstall
                ? () => onAiInstall({ tool, appName: cliTitle })
                : undefined
            }
          />
        )}
        {desktop && (
          <DesktopLifecycleRow
            app={desktop}
            refreshRequestId={refreshRequestId}
            onAiInstall={
              onAiInstall
                ? () => onAiInstall({
                    tool: desktop,
                    appName: desktop === "codex-desktop" ? "Codex Desktop" : "Claude Desktop",
                    supportsCustomLocation: false,
                    supportsVersionPin: false,
                  })
                : undefined
            }
          />
        )}
      </div>
    </section>
  );
}
