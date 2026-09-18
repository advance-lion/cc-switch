import { useCallback, useEffect, useRef, useState } from "react";
import {
  Download,
  ChevronDown,
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
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
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
  type CliLifecycleJob,
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
  | "pi"
  | "dsh";

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
  dsh: "dsh",
};

const DESKTOP_BY_APP: Partial<Record<AppId, DesktopAppId>> = {
  claude: "claude-desktop",
  "claude-desktop": "claude-desktop",
  codex: "codex-desktop",
  hermes: "hermes-desktop",
};

const cliCache = new Map<
  RuntimeTool,
  {
    status: ToolVersion | null;
    capabilities: ToolLifecycleCapabilities | null;
  }
>();
const desktopCache = new Map<DesktopAppId, DesktopAppStatus>();

type CliRefreshResult = {
  status: ToolVersion | null;
  capabilities: ToolLifecycleCapabilities | null;
};
type DesktopRefreshResult = DesktopAppStatus | null;

const cliRefreshInFlight = new Map<RuntimeTool, Promise<CliRefreshResult>>();
const desktopRefreshInFlight = new Map<
  DesktopAppId,
  Promise<DesktopRefreshResult>
>();

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
  launchIcon?: "terminal" | "monitorPlay";
  onInstall: () => void;
  onAiInstall?: () => void;
  onLaunch: () => void;
  onCheckOrUpdate: () => void;
  onUninstall: () => void;
  onRedetect: () => void;
  onCancel?: () => void;
}

/** CLI 与桌面任务共用的横幅视图：渲染只依赖状态、动作、错误和日志尾部。 */
interface LifecycleJobView {
  state: string;
  action: string;
  errorMessage: string | null;
  logs: Array<{ at: number; step: string; message: string }>;
}

/** 进行中任务横幅：不定态进度条 + 实时日志尾部（软件商店式安装进度）。 */
function JobActiveBanner({ job }: { job: LifecycleJobView }) {
  const { t } = useTranslation();
  return (
    <div className="rounded-lg border border-blue-500/25 bg-blue-500/5 px-3 py-2 text-xs text-foreground">
      <div className="flex items-center gap-2 font-medium">
        <Loader2 className="h-3.5 w-3.5 animate-spin text-blue-600" />
        {job.state === "verifying"
          ? t("appLifecycle.jobVerifying")
          : t("appLifecycle.jobRunning")}
      </div>
      <div className="mt-2 h-1 overflow-hidden rounded-full bg-blue-500/15">
        <div className="cc-progress-indeterminate h-full w-1/3 rounded-full bg-blue-500" />
      </div>
      <div className="mt-2 space-y-1 text-muted-foreground" aria-live="polite">
        {job.logs.slice(-4).map((entry, index) => (
          <p key={`${entry.at}-${entry.step}-${index}`}>{entry.message}</p>
        ))}
      </div>
    </div>
  );
}

/** 失败/中断任务横幅：给出动作语义、错误详情和日志尾部。 */
function JobFailureBanner({ job }: { job: LifecycleJobView }) {
  const { t } = useTranslation();
  return (
    <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-xs text-amber-800 dark:text-amber-200">
      {t(
        job.state === "interrupted"
          ? "appLifecycle.lastJobInterrupted"
          : "appLifecycle.lastJobFailed",
        {
          action: t(
            job.action === "install"
              ? "appLifecycle.actionInstall"
              : job.action === "update"
                ? "appLifecycle.actionUpdate"
                : "appLifecycle.actionUninstall",
          ),
        },
      )}
      {job.errorMessage
        ? `：${job.errorMessage}`
        : t("appLifecycle.retryAfterDetect")}
      {job.logs.length > 0 && (
        <div className="mt-2 space-y-1 border-t border-amber-500/20 pt-2 opacity-90">
          {job.logs.slice(-4).map((entry, index) => (
            <p key={`${entry.at}-${entry.step}-${index}`}>{entry.message}</p>
          ))}
        </div>
      )}
    </div>
  );
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
  launchIcon = "terminal",
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
            {additionalInstallationCount > 0
              ? ` · 另检测到 ${additionalInstallationCount} 份安装`
              : ""}
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
            {t("appLifecycle.stop")}
          </Button>
        )}
        {installed ? (
          <>
            <Button
              variant="outline"
              size="sm"
              disabled={busy || needsRepair || !canLaunch}
              onClick={onLaunch}
              title={!canLaunch ? (disabledReason ?? undefined) : undefined}
            >
              {action === "launch" ? (
                <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
              ) : launchIcon === "monitorPlay" ? (
                <MonitorPlay className="mr-1.5 h-3.5 w-3.5" />
              ) : (
                <Terminal className="mr-1.5 h-3.5 w-3.5" />
              )}
              {launchLabel}
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={
                busy || !canUpdate || (updateChecked && !updateAvailable)
              }
              onClick={onCheckOrUpdate}
              title={!canUpdate ? (disabledReason ?? undefined) : undefined}
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
              title={!canInstall ? (disabledReason ?? undefined) : undefined}
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
  const [status, setStatus] = useState<ToolVersion | null>(
    cached?.status ?? null,
  );
  const [capabilities, setCapabilities] =
    useState<ToolLifecycleCapabilities | null>(cached?.capabilities ?? null);
  const [loading, setLoading] = useState(!cached);
  const [action, setAction] = useState<string | null>(null);
  const [uninstallOpen, setUninstallOpen] = useState(false);
  const [uninstallCompletedOpen, setUninstallCompletedOpen] = useState(false);
  const [lastJob, setLastJob] = useState<CliLifecycleJob | null>(null);
  const [activeJobId, setActiveJobId] = useState<string | null>(null);
  const [activeJob, setActiveJob] = useState<CliLifecycleJob | null>(null);
  const lastRefreshRequestRef = useRef(refreshRequestId);

  const refresh = useCallback(
    async (includeLatest = false) => {
      // Only "check for updates" (includeLatest) calls are slow enough to
      // warrant in-flight deduplication. Local version detection is fast.
      if (includeLatest) {
        const existing = cliRefreshInFlight.get(tool);
        if (existing) {
          setLoading(true);
          try {
            const result = await existing;
            setStatus(result.status);
            setCapabilities(result.capabilities);
            return result.status;
          } finally {
            setLoading(false);
          }
        }
      }

      setLoading(true);
      const promise = (async (): Promise<CliRefreshResult> => {
        try {
          const [versions, lifecycleCapabilities] = await Promise.all([
            includeLatest
              ? settingsApi.checkToolUpdates([tool])
              : settingsApi.getToolVersions([tool]),
            settingsApi.getToolLifecycleCapabilities([tool]),
          ]);
          const next = versions[0] ?? null;
          const nextCapabilities = lifecycleCapabilities[0] ?? null;
          cliCache.set(tool, { status: next, capabilities: nextCapabilities });
          return { status: next, capabilities: nextCapabilities };
        } catch (error) {
          toast.error(t("appLifecycle.detectFailed"), {
            description: extractErrorMessage(error),
          });
          return { status: null, capabilities: null };
        }
      })();

      if (includeLatest) cliRefreshInFlight.set(tool, promise);
      try {
        const result = await promise;
        setStatus(result.status);
        setCapabilities(result.capabilities);
        return result.status;
      } finally {
        setLoading(false);
        if (includeLatest) cliRefreshInFlight.delete(tool);
      }
    },
    [t, tool],
  );

  // On remount after agent switch: if a refresh is still in-flight,
  // restore the loading indicator and apply the result when it settles.
  useEffect(() => {
    const inFlight = cliRefreshInFlight.get(tool);
    if (!inFlight) return;
    let cancelled = false;
    setLoading(true);
    inFlight
      .then((result) => {
        if (cancelled) return;
        setStatus(result.status);
        setCapabilities(result.capabilities);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [tool]);

  useEffect(() => {
    if (!cliCache.has(tool)) void refresh(false);
    void settingsApi
      .listCliLifecycleJobs(tool)
      .then((jobs) => {
        const latest = jobs[0] ?? null;
        if (
          latest &&
          ["queued", "running", "verifying"].includes(latest.state)
        ) {
          // 切换页面后重新挂载：恢复进行中任务的进度展示与取消入口，
          // 不再出现"后台还在装、按钮却已变回可点"的假空闲状态。
          setActiveJobId(latest.id);
          setAction(latest.action);
          setActiveJob(latest);
        }
        setLastJob(latest);
      })
      .catch(() => undefined);
  }, [refresh, tool]);

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
      const job = await settingsApi
        .getCliLifecycleJob(activeJobId)
        .catch(() => null);
      if (!disposed && job) setActiveJob(job);
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 700);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [activeJobId]);

  const runLifecycle = async (nextAction: "install" | "update") => {
    const jobId = crypto.randomUUID();
    setActiveJobId(jobId);
    setActiveJob(null);
    setAction(nextAction);
    try {
      await settingsApi.runCliLifecycleAction(tool, nextAction, jobId);
      const detected = await refresh(false);
      if (nextAction === "install" && !detected?.version) {
        throw new Error(t("appLifecycle.installNotDetected"));
      }
      const jobs = await settingsApi.listCliLifecycleJobs(tool).catch(() => []);
      setLastJob(jobs[0] ?? null);
      toast.success(
        t(
          nextAction === "install"
            ? "appLifecycle.installCompleted"
            : "appLifecycle.updateCompleted",
        ),
      );
    } catch (error) {
      const jobs = await settingsApi.listCliLifecycleJobs(tool).catch(() => []);
      setLastJob(jobs[0] ?? null);
      toast.error(
        t(
          nextAction === "install"
            ? "appLifecycle.installFailed"
            : "appLifecycle.updateFailed",
        ),
        { description: extractErrorMessage(error) },
      );
    } finally {
      setActiveJobId(null);
      setAction(null);
    }
  };

  const cancelLifecycle = async () => {
    if (!activeJobId) return;
    const accepted = await settingsApi.cancelCliLifecycleJob(activeJobId);
    if (!accepted) {
      toast.error(t("appLifecycle.cancelFinished"));
      await refresh(false);
    }
  };

  const launch = async () => {
    setAction("launch");
    try {
      if (tool === "dsh") {
        await settingsApi.launchDsh();
        toast.success(t("appLifecycle.dshStarted"));
      } else {
        await settingsApi.launchToolTerminal(tool);
        toast.success(t("appLifecycle.terminalOpened"));
      }
    } catch (error) {
      toast.error(
        t(
          tool === "dsh"
            ? "appLifecycle.dshLaunchFailed"
            : "appLifecycle.launchFailed",
        ),
        { description: extractErrorMessage(error) },
      );
    } finally {
      setAction(null);
    }
  };

  const restart = async () => {
    setAction("launch");
    try {
      await settingsApi.restartDsh();
      toast.success(t("appLifecycle.dshRestarted"));
    } catch (error) {
      toast.error(t("appLifecycle.dshRestartFailed"), {
        description: extractErrorMessage(error),
      });
    } finally {
      setAction(null);
    }
  };

  const uninstall = async () => {
    const jobId = crypto.randomUUID();
    setActiveJobId(jobId);
    setActiveJob(null);
    setAction("uninstall");
    try {
      await settingsApi.runCliLifecycleAction(tool, "uninstall", jobId);
      const detected = await refresh(false);
      if (detected?.version)
        throw new Error(t("appLifecycle.uninstallStillDetected"));
      const jobs = await settingsApi.listCliLifecycleJobs(tool).catch(() => []);
      setLastJob(jobs[0] ?? null);
      setUninstallOpen(false);
      setUninstallCompletedOpen(true);
    } catch (error) {
      const jobs = await settingsApi.listCliLifecycleJobs(tool).catch(() => []);
      setLastJob(jobs[0] ?? null);
      toast.error(t("appLifecycle.uninstallFailed"), {
        description: extractErrorMessage(error),
      });
    } finally {
      setActiveJobId(null);
      setAction(null);
    }
  };

  const installed = Boolean(status?.version);
  const updateChecked = Boolean(status?.latest_version);
  const updateAvailable = Boolean(
    status?.version &&
      status.latest_version &&
      isUpdateAvailable(status.version, status.latest_version),
  );
  const busy = action !== null;

  return (
    <>
      <div className="space-y-1.5">
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
          launchLabel={t(
            tool === "dsh" ? "appLifecycle.dshLaunch" : "appLifecycle.launch",
          )}
          launchIcon={tool === "dsh" ? "monitorPlay" : "terminal"}
          onInstall={() => void runLifecycle("install")}
          onAiInstall={onAiInstall}
          onLaunch={() => void launch()}
          onCheckOrUpdate={() =>
            void (updateChecked && updateAvailable
              ? runLifecycle("update")
              : refresh(true))
          }
          onUninstall={() => setUninstallOpen(true)}
          onRedetect={() => void refresh(false)}
          onCancel={activeJobId ? () => void cancelLifecycle() : undefined}
        />
        {tool === "dsh" && installed && !busy && (
          <button
            onClick={() => void restart()}
            className="inline-flex items-center gap-1.5 rounded-md border border-border-default px-2.5 py-1 text-xs font-medium text-muted-foreground hover:bg-accent/50"
          >
            <RefreshCw className="h-3.5 w-3.5" />
            {t("appLifecycle.restart")}
          </button>
        )}
        {activeJob &&
          ["queued", "running", "verifying"].includes(activeJob.state) && (
            <JobActiveBanner job={activeJob} />
          )}
        {lastJob && ["failed", "interrupted"].includes(lastJob.state) && (
          <JobFailureBanner job={lastJob} />
        )}
      </div>
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
  const [uninstallCompletionMessage, setUninstallCompletionMessage] = useState<
    string | null
  >(null);
  const [lastJob, setLastJob] = useState<DesktopLifecycleJob | null>(null);
  const [activeJobId, setActiveJobId] = useState<string | null>(null);
  const [activeJob, setActiveJob] = useState<DesktopLifecycleJob | null>(null);
  const lastRefreshRequestRef = useRef(refreshRequestId);

  const refresh = useCallback(
    async (includeLatest = false) => {
      // Only "check for updates" (includeLatest) calls are slow enough to
      // warrant in-flight deduplication. Local status detection is fast.
      if (includeLatest) {
        const existing = desktopRefreshInFlight.get(app);
        if (existing) {
          setLoading(true);
          try {
            const next = await existing;
            if (next) setStatus(next);
            return next;
          } finally {
            setLoading(false);
          }
        }
      }

      setLoading(true);
      const promise = (async (): Promise<DesktopRefreshResult> => {
        try {
          const next = includeLatest
            ? await settingsApi.checkDesktopAppUpdates(app)
            : await settingsApi.getDesktopAppStatus(app);
          desktopCache.set(app, next);
          return next;
        } catch (error) {
          toast.error(t("appLifecycle.detectFailed"), {
            description: extractErrorMessage(error),
          });
          return null;
        }
      })();

      if (includeLatest) desktopRefreshInFlight.set(app, promise);
      try {
        const next = await promise;
        if (next) setStatus(next);
        return next;
      } finally {
        setLoading(false);
        if (includeLatest) desktopRefreshInFlight.delete(app);
      }
    },
    [app, t],
  );

  // On remount after agent switch: if a refresh is still in-flight,
  // restore the loading indicator and apply the result when it settles.
  useEffect(() => {
    const inFlight = desktopRefreshInFlight.get(app);
    if (!inFlight) return;
    let cancelled = false;
    setLoading(true);
    inFlight
      .then((next) => {
        if (cancelled) return;
        if (next) setStatus(next);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [app]);

  useEffect(() => {
    if (!desktopCache.has(app)) void refresh(false);
    void settingsApi
      .listDesktopLifecycleJobs(app)
      .then((jobs) => {
        const latest = jobs[0] ?? null;
        if (
          latest &&
          ["queued", "running", "verifying"].includes(latest.state)
        ) {
          // 切换页面后重新挂载：恢复进行中任务的进度展示与取消入口。
          setActiveJobId(latest.id);
          setAction(latest.action);
          setActiveJob(latest);
        }
        setLastJob(latest);
      })
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
      const job = await settingsApi
        .getDesktopLifecycleJob(activeJobId)
        .catch(() => null);
      if (!disposed && job) setActiveJob(job);
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 700);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [activeJobId]);

  const runLifecycle = async (
    nextAction: "install" | "update" | "uninstall",
  ) => {
    const jobId = crypto.randomUUID();
    setActiveJobId(jobId);
    setActiveJob(null);
    setAction(nextAction);
    try {
      const next = await settingsApi.runDesktopAppLifecycleAction(
        app,
        nextAction,
        jobId,
      );
      setStatus(next);
      desktopCache.set(app, next);
      const jobs = await settingsApi
        .listDesktopLifecycleJobs(app)
        .catch(() => []);
      setLastJob(jobs[0] ?? null);
      if (nextAction === "uninstall") {
        setUninstallOpen(false);
        setUninstallCompletionMessage(next.installed ? next.reason : null);
        setUninstallCompletedOpen(true);
      } else {
        toast.success(
          t(
            nextAction === "install"
              ? "appLifecycle.installCompleted"
              : "appLifecycle.updateCompleted",
          ),
        );
      }
    } catch (error) {
      const jobs = await settingsApi
        .listDesktopLifecycleJobs(app)
        .catch(() => []);
      setLastJob(jobs[0] ?? null);
      const key =
        nextAction === "install"
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
      toast.error(t("appLifecycle.cancelFinished"));
      await refresh(false);
    }
  };

  const launch = async () => {
    setAction("launch");
    try {
      await settingsApi.launchDesktopApp(app);
      toast.success(
        t("appLifecycle.desktopOpened", { app: status?.display_name ?? app }),
      );
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
    status?.version &&
      status.latest_version &&
      isUpdateAvailable(status.version, status.latest_version),
  );
  const title =
    status?.display_name ??
    (app === "codex-desktop"
      ? "Codex Desktop"
      : app === "hermes-desktop"
        ? "Hermes Desktop"
        : "Claude Desktop");

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
          additionalInstallationCount={Math.max(
            0,
            (status?.installations?.length ?? 0) - 1,
          )}
          loading={loading}
          busy={action !== null}
          action={action}
          canInstall={status?.can_install ?? false}
          canUpdate={status?.can_update ?? false}
          canUninstall={status?.can_uninstall ?? false}
          canLaunch={status?.can_launch ?? false}
          disabledReason={status?.reason}
          launchLabel={t("appLifecycle.launchDesktop")}
          launchIcon="monitorPlay"
          onInstall={() => void runLifecycle("install")}
          onAiInstall={onAiInstall}
          onLaunch={() => void launch()}
          onCheckOrUpdate={() =>
            void (updateChecked && updateAvailable
              ? runLifecycle("update")
              : refresh(true))
          }
          onUninstall={() => setUninstallOpen(true)}
          onRedetect={() => void refresh(false)}
          onCancel={activeJobId ? () => void cancelLifecycle() : undefined}
        />
        {activeJob &&
          ["queued", "running", "verifying"].includes(activeJob.state) && (
            <JobActiveBanner job={activeJob} />
          )}
        {lastJob && ["failed", "interrupted"].includes(lastJob.state) && (
          <JobFailureBanner job={lastJob} />
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
            {message ??
              t("appLifecycle.uninstallCompletedDescription", { app: appName })}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter className="pt-2 sm:justify-end">
          <Button onClick={() => onOpenChange(false)}>
            {t("common.close")}
          </Button>
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
  const [isExpanded, setIsExpanded] = useState(true);
  const app = APP_ICON_MAP[appId];
  const tool = TOOL_BY_APP[appId];
  const desktop = DESKTOP_BY_APP[appId];
  const cliTitle =
    appId === "codex"
      ? "Codex CLI"
      : appId === "claude"
        ? "Claude Code CLI"
        : `${app.label} CLI`;

  return (
    <section className="sticky top-0 z-20 rounded-2xl border bg-background/95 p-4 shadow-sm backdrop-blur supports-[backdrop-filter]:bg-background/85">
      <Collapsible open={isExpanded} onOpenChange={setIsExpanded}>
        <CollapsibleTrigger
          className="flex w-full items-start justify-between gap-3 text-left"
          aria-label={t(
            isExpanded
              ? "appLifecycle.collapseDetails"
              : "appLifecycle.expandDetails",
          )}
        >
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
          <ChevronDown
            className={`mt-1 h-4 w-4 shrink-0 text-muted-foreground transition-transform ${
              isExpanded ? "rotate-180" : ""
            }`}
            aria-hidden="true"
          />
        </CollapsibleTrigger>

        <CollapsibleContent className="pt-3">
          <div className="space-y-2.5">
            {tool && (
              <CliLifecycleRow
                tool={tool}
                title={cliTitle}
                subtitle={t(
                  tool === "dsh"
                    ? "appLifecycle.dshDescription"
                    : "appLifecycle.cliDescription",
                )}
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
                    ? () =>
                        onAiInstall({
                          tool: desktop,
                          appName:
                            desktop === "codex-desktop"
                              ? "Codex Desktop"
                              : desktop === "hermes-desktop"
                                ? "Hermes Desktop"
                                : "Claude Desktop",
                          supportsCustomLocation: false,
                          supportsVersionPin: false,
                        })
                    : undefined
                }
              />
            )}
          </div>
        </CollapsibleContent>
      </Collapsible>
    </section>
  );
}
