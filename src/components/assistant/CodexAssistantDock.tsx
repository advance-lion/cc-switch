import { useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import {
  Bot,
  Check,
  ChevronRight,
  CircleAlert,
  CircleStop,
  FolderOpen,
  Loader2,
  MessageCircle,
  Send,
  Terminal,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { CodexIcon } from "@/components/BrandIcons";
import { cn } from "@/lib/utils";
import {
  isCodexAssistantWebBridgeActive,
  settingsApi,
  type CodexAssistantPlan,
} from "@/lib/api";
import { extractErrorMessage } from "@/utils/errorUtils";

type CodexRuntime = {
  version: string | null;
  installed_but_broken: boolean;
};

type FloatingPosition = {
  x: number;
  y: number;
};

type InstallLocationMode = "default" | "custom";
type InstallVersionMode = "stable" | "latest" | "specific";

// Keep the visual affordance close to the Codex mark, but retain enough room
// for a reliable pointer target on desktop.
const FLOATING_BUTTON_SIZE = 34;
const FLOATING_BUTTON_MARGIN = 16;
const FLOATING_POSITION_STORAGE_KEY = "cc-switch-codex-assistant-position";
const INSTALL_DIRECTORY_STORAGE_KEY =
  "cc-switch-codex-assistant-install-directory";
const MAX_VISIBLE_LOG_LINES = 180;

const clampFloatingPosition = (
  position: FloatingPosition,
): FloatingPosition => ({
  x: Math.min(
    Math.max(FLOATING_BUTTON_MARGIN, position.x),
    Math.max(
      FLOATING_BUTTON_MARGIN,
      window.innerWidth - FLOATING_BUTTON_SIZE - FLOATING_BUTTON_MARGIN,
    ),
  ),
  y: Math.min(
    Math.max(FLOATING_BUTTON_MARGIN, position.y),
    Math.max(
      FLOATING_BUTTON_MARGIN,
      window.innerHeight - FLOATING_BUTTON_SIZE - FLOATING_BUTTON_MARGIN,
    ),
  ),
});

const defaultFloatingPosition = (): FloatingPosition =>
  clampFloatingPosition({
    x: window.innerWidth - FLOATING_BUTTON_SIZE - FLOATING_BUTTON_MARGIN,
    y: window.innerHeight / 2 - FLOATING_BUTTON_SIZE / 2,
  });

export interface CodexAssistantInstallIntent {
  tool: string;
  appName: string;
  supportsCustomLocation?: boolean;
  supportsVersionPin?: boolean;
}

interface CodexAssistantDockProps {
  providerReady: boolean;
  onOpenCodexConfiguration: () => void;
  onDismiss?: () => void;
  onInstallationCompleted?: (tool: string) => void;
  installIntent?: CodexAssistantInstallIntent | null;
  /** Incremented by external entry points such as “Add custom Agent”. */
  openRequestId?: number;
}

/**
 * 右侧 Codex 安装助手。
 *
 * 它是受控安装流的入口，而不是一个把聊天文本拼进 shell 的终端。真正的执行器接入后，
 * 只会接受“生成安装计划 / 用户确认 / 查看日志”这类结构化操作。
 */
export function CodexAssistantDock({
  providerReady,
  onOpenCodexConfiguration,
  onDismiss,
  onInstallationCompleted,
  installIntent,
  openRequestId = 0,
}: CodexAssistantDockProps) {
  const { t } = useTranslation();
  const webBridgeActive = isCodexAssistantWebBridgeActive();
  const [open, setOpen] = useState(false);
  const [showFloatingGreeting, setShowFloatingGreeting] = useState(true);
  const [runtime, setRuntime] = useState<CodexRuntime | null>(null);
  const supportsCustomInstallLocation = installIntent?.supportsCustomLocation !== false;
  const supportsVersionPin = installIntent?.supportsVersionPin !== false;
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [request, setRequest] = useState("");
  const [targetDirectory, setTargetDirectory] = useState("");
  const [installLocationMode, setInstallLocationMode] =
    useState<InstallLocationMode>("default");
  const [installVersionMode, setInstallVersionMode] =
    useState<InstallVersionMode>("stable");
  const [specificVersion, setSpecificVersion] = useState("");
  const [webDirectoryEditorOpen, setWebDirectoryEditorOpen] = useState(false);
  const [webDirectoryDraft, setWebDirectoryDraft] = useState("");
  const [plan, setPlan] = useState<CodexAssistantPlan | null>(null);
  const [planId, setPlanId] = useState<string | null>(null);
  const [logs, setLogs] = useState<string[]>([]);
  const [runError, setRunError] = useState<string | null>(null);
  const [planning, setPlanning] = useState(false);
  const [executing, setExecuting] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [activeRunId, setActiveRunId] = useState<string | null>(null);
  const [floatingPosition, setFloatingPosition] =
    useState<FloatingPosition | null>(null);
  const floatingDockRef = useRef<HTMLDivElement>(null);
  const latestFloatingPositionRef = useRef<FloatingPosition | null>(null);
  const pendingFloatingPositionRef = useRef<FloatingPosition | null>(null);
  const dragAnimationFrameRef = useRef<number | null>(null);
  const dragStateRef = useRef<{
    pointerId: number;
    startClientX: number;
    startClientY: number;
    startPosition: FloatingPosition;
    moved: boolean;
  } | null>(null);
  const suppressOpenRef = useRef(false);
  const activeRunIdRef = useRef<string | null>(null);
  const activeRunKindRef = useRef<"plan" | "execute" | null>(null);
  const activeInstallToolRef = useRef<string | null>(null);
  const planReceivedRef = useRef(false);
  const lastHandledOpenRequestRef = useRef(0);

  const appendLog = useCallback((line: string) => {
    setLogs((current) => [...current, line].slice(-MAX_VISIBLE_LOG_LINES));
  }, []);

  useEffect(() => {
    const storedPosition = localStorage.getItem(FLOATING_POSITION_STORAGE_KEY);
    if (storedPosition) {
      try {
        const parsed = JSON.parse(storedPosition) as Partial<FloatingPosition>;
        if (typeof parsed.x === "number" && typeof parsed.y === "number") {
          const next = clampFloatingPosition(parsed as FloatingPosition);
          latestFloatingPositionRef.current = next;
          setFloatingPosition(next);
          return;
        }
      } catch {
        // A stale or malformed stored value simply falls back to the docked default.
      }
    }
    const next = defaultFloatingPosition();
    latestFloatingPositionRef.current = next;
    setFloatingPosition(next);
  }, []);

  useEffect(() => {
    const storedDirectory = localStorage.getItem(INSTALL_DIRECTORY_STORAGE_KEY);
    if (storedDirectory) setTargetDirectory(storedDirectory);
  }, []);

  useEffect(() => {
    if (
      openRequestId <= 0 ||
      openRequestId === lastHandledOpenRequestRef.current
    ) {
      return;
    }
    lastHandledOpenRequestRef.current = openRequestId;
    if (installIntent) {
      setRequest(
        t("codexAssistant.installRequest", { app: installIntent.appName }),
      );
      setInstallLocationMode("default");
      setInstallVersionMode("stable");
      setSpecificVersion("");
    }
    setShowFloatingGreeting(false);
    setOpen(true);
  }, [installIntent, openRequestId, t]);

  useEffect(() => {
    const keepFloatingButtonVisible = () => {
      const next = clampFloatingPosition(
        latestFloatingPositionRef.current ?? defaultFloatingPosition(),
      );
      latestFloatingPositionRef.current = next;
      setFloatingPosition(next);
    };
    window.addEventListener("resize", keepFloatingButtonVisible);
    return () =>
      window.removeEventListener("resize", keepFloatingButtonVisible);
  }, []);

  useEffect(
    () => () => {
      if (dragAnimationFrameRef.current !== null) {
        cancelAnimationFrame(dragAnimationFrameRef.current);
      }
    },
    [],
  );

  const refreshRuntime = useCallback(async () => {
    setChecking(true);
    try {
      const [next] = await settingsApi.getToolVersions(["codex"]);
      setRuntime(next ?? null);
    } catch (error) {
      toast.error(t("codexAssistant.runtimeCheckFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    } finally {
      setChecking(false);
    }
  }, [t]);

  useEffect(() => {
    let dispose = false;
    let unlisten: (() => void) | undefined;

    void settingsApi
      .onCodexAssistantEvent((event) => {
        const knownRunId = activeRunIdRef.current;
        // The Tauri command returns the run id after the process has started. In
        // the unlikely event that the first event wins that race, adopt its id.
        if (!knownRunId && event.kind === "started") {
          activeRunIdRef.current = event.runId;
          setActiveRunId(event.runId);
        } else if (knownRunId !== event.runId) {
          return;
        }

        if (event.kind === "log" || event.kind === "stderr") {
          const prefix = event.kind === "stderr" ? "stderr · " : "";
          if (event.message) appendLog(`${prefix}${event.message}`);
          return;
        }

        if (event.kind === "plan" && event.plan && event.planId) {
          planReceivedRef.current = true;
          setPlan(event.plan);
          setPlanId(event.planId);
          return;
        }

        if (event.kind === "finished") {
          const completedKind = activeRunKindRef.current;
          const success = event.success === true;
          activeRunIdRef.current = null;
          activeRunKindRef.current = null;
          setActiveRunId(null);
          setPlanning(false);
          setExecuting(false);
          setStopping(false);

          if (!success) {
            const message = event.message || t("codexAssistant.runFailed");
            activeInstallToolRef.current = null;
            setRunError(message);
            toast.error(t("codexAssistant.runFailed"), {
              description: message,
            });
          } else if (completedKind === "plan" && !planReceivedRef.current) {
            setRunError(t("codexAssistant.invalidPlan"));
          } else if (completedKind === "execute") {
            void refreshRuntime();
            const tool = activeInstallToolRef.current;
            if (tool) onInstallationCompleted?.(tool);
            activeInstallToolRef.current = null;
            toast.success(t("codexAssistant.executionCompleted"));
          }
        }
      })
      .then((listener) => {
        if (dispose) listener();
        else unlisten = listener;
      });

    return () => {
      dispose = true;
      unlisten?.();
    };
  }, [appendLog, onInstallationCompleted, refreshRuntime, t]);

  useEffect(() => {
    if (open) void refreshRuntime();
  }, [open, refreshRuntime]);

  const installCodex = async () => {
    setInstalling(true);
    try {
      await settingsApi.runToolLifecycleAction(["codex"], "install");
      await refreshRuntime();
      toast.success(t("codexAssistant.runtimeInstalled"));
    } catch (error) {
      toast.error(t("codexAssistant.runtimeInstallFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    } finally {
      setInstalling(false);
    }
  };

  const cliReady = Boolean(runtime?.version && !runtime?.installed_but_broken);
  // In Vite development, the bridge uses the current local Codex CLI config.
  // A failed CLI request remains the source of truth for provider validity.
  const effectiveProviderReady = providerReady || webBridgeActive;
  const ready = cliReady && effectiveProviderReady;
  const blockedReason = !cliReady
    ? t("codexAssistant.cliRequired")
    : !effectiveProviderReady
      ? t("codexAssistant.providerRequired")
      : null;

  const chooseInstallDirectory = async () => {
    try {
      if (webBridgeActive) {
        setWebDirectoryDraft(targetDirectory);
        setWebDirectoryEditorOpen(true);
        return;
      }
      const selected = await settingsApi.pickDirectory(
        targetDirectory || undefined,
      );
      if (!selected) return;
      setTargetDirectory(selected);
      localStorage.setItem(INSTALL_DIRECTORY_STORAGE_KEY, selected);
    } catch (error) {
      toast.error(t("codexAssistant.directorySelectionFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    }
  };

  const saveWebDirectory = () => {
    const selected = webDirectoryDraft.trim();
    if (!selected) {
      toast.error(t("codexAssistant.directoryRequired"));
      return;
    }
    setTargetDirectory(selected);
    localStorage.setItem(INSTALL_DIRECTORY_STORAGE_KEY, selected);
    setWebDirectoryEditorOpen(false);
  };

  const requestPlan = async () => {
    if (!request.trim() || planning || executing) return;
    if (
      (!installIntent || (supportsCustomInstallLocation && installLocationMode === "custom")) &&
      !targetDirectory
    ) {
      toast.error(t("codexAssistant.directoryRequired"));
      return;
    }
    if (
      installIntent && supportsVersionPin &&
      installVersionMode === "specific" &&
      !specificVersion.trim()
    ) {
      toast.error(t("codexAssistant.versionRequired"));
      return;
    }
    setPlan(null);
    setPlanId(null);
    setLogs([]);
    setRunError(null);
    planReceivedRef.current = false;
    activeRunKindRef.current = "plan";
    setPlanning(true);
    try {
      const runId = await settingsApi.startCodexAssistantPlan(
        request.trim(),
        targetDirectory,
        installIntent
          ? {
              tool: installIntent.tool,
              requestedVersion:
                supportsVersionPin && installVersionMode === "specific"
                  ? specificVersion.trim()
                  : installVersionMode,
              customInstallLocation:
                supportsCustomInstallLocation && installLocationMode === "custom",
            }
          : undefined,
      );
      if (activeRunKindRef.current === "plan") {
        activeRunIdRef.current = runId;
        setActiveRunId(runId);
      }
    } catch (error) {
      activeRunKindRef.current = null;
      setPlanning(false);
      const message =
        extractErrorMessage(error) || t("codexAssistant.runFailed");
      setRunError(message);
      toast.error(t("codexAssistant.runFailed"), { description: message });
    }
  };

  const executePlan = async () => {
    if (!planId || !plan?.executable || planning || executing) return;
    setRunError(null);
    activeInstallToolRef.current = plan.install?.tool ?? null;
    activeRunKindRef.current = "execute";
    setExecuting(true);
    try {
      const runId = await settingsApi.executeCodexAssistantPlan(planId);
      if (activeRunKindRef.current === "execute") {
        activeRunIdRef.current = runId;
        setActiveRunId(runId);
      }
    } catch (error) {
      activeRunKindRef.current = null;
      activeInstallToolRef.current = null;
      setExecuting(false);
      const message =
        extractErrorMessage(error) || t("codexAssistant.runFailed");
      setRunError(message);
      toast.error(t("codexAssistant.runFailed"), { description: message });
    }
  };

  const cancelRun = async () => {
    const runId = activeRunIdRef.current;
    if (!runId || stopping) return;
    setStopping(true);
    try {
      await settingsApi.cancelCodexAssistantRun(runId);
      appendLog(t("codexAssistant.cancelling"));
    } catch (error) {
      setStopping(false);
      toast.error(t("codexAssistant.cancelFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    }
  };

  const persistFloatingPosition = (position: FloatingPosition) => {
    localStorage.setItem(
      FLOATING_POSITION_STORAGE_KEY,
      JSON.stringify(position),
    );
  };

  const moveFloatingButton = (position: FloatingPosition) => {
    latestFloatingPositionRef.current = position;
    pendingFloatingPositionRef.current = position;
    if (dragAnimationFrameRef.current !== null) return;

    dragAnimationFrameRef.current = requestAnimationFrame(() => {
      const next = pendingFloatingPositionRef.current;
      const dock = floatingDockRef.current;
      if (next && dock) {
        dock.style.transform = `translate3d(${next.x}px, ${next.y}px, 0)`;
      }
      dragAnimationFrameRef.current = null;
    });
  };

  const handlePointerDown = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    const current =
      latestFloatingPositionRef.current ??
      floatingPosition ??
      defaultFloatingPosition();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragStateRef.current = {
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startClientY: event.clientY,
      startPosition: current,
      moved: false,
    };
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const drag = dragStateRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    event.preventDefault();
    const deltaX = event.clientX - drag.startClientX;
    const deltaY = event.clientY - drag.startClientY;
    if (Math.abs(deltaX) + Math.abs(deltaY) > 4) drag.moved = true;
    if (!drag.moved) return;
    moveFloatingButton(
      clampFloatingPosition({
        x: drag.startPosition.x + deltaX,
        y: drag.startPosition.y + deltaY,
      }),
    );
  };

  const finishDrag = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const drag = dragStateRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    dragStateRef.current = null;
    if (!drag.moved) return;
    const nextPosition = clampFloatingPosition({
      x: drag.startPosition.x + event.clientX - drag.startClientX,
      y: drag.startPosition.y + event.clientY - drag.startClientY,
    });
    moveFloatingButton(nextPosition);
    setFloatingPosition(nextPosition);
    persistFloatingPosition(nextPosition);
    // Browsers fire a click after pointerup. A drag must not open the panel.
    suppressOpenRef.current = true;
  };

  const openAssistant = () => {
    if (suppressOpenRef.current) {
      suppressOpenRef.current = false;
      return;
    }
    setShowFloatingGreeting(false);
    setOpen(true);
  };

  const closeAssistant = () => {
    setOpen(false);
    onDismiss?.();
  };

  // The assistant intentionally has no click-blocking page overlay, so Escape
  // is the reliable second exit when it is opened above another dialog.
  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeAssistant();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [open, onDismiss]);

  return (
    <>
      <div
        ref={floatingDockRef}
        style={
          floatingPosition
            ? {
                left: 0,
                top: 0,
                transform: `translate3d(${floatingPosition.x}px, ${floatingPosition.y}px, 0)`,
              }
            : { right: FLOATING_BUTTON_MARGIN, top: "calc(50% - 17px)" }
        }
        className="fixed z-[120] isolate will-change-transform"
      >
        {showFloatingGreeting && (
          <div className="pointer-events-none absolute right-[calc(100%+10px)] top-1/2 hidden w-56 -translate-y-1/2 rounded-2xl border border-violet-500/25 bg-background/75 py-2 pl-3 pr-8 text-left text-xs leading-relaxed text-foreground/75 opacity-90 shadow-[0_8px_24px_-16px_rgba(76,29,149,0.45)] backdrop-blur-[2px] sm:block">
            <span>{t("codexAssistant.floatingGreeting")}</span>
            <button
              type="button"
              className="pointer-events-auto absolute right-1.5 top-1.5 flex h-5 w-5 items-center justify-center rounded-full text-muted-foreground/70 transition-colors hover:bg-violet-500/10 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-violet-500/40"
              onClick={() => setShowFloatingGreeting(false)}
              aria-label={t("codexAssistant.dismissGreeting")}
              title={t("codexAssistant.dismissGreeting")}
            >
              <X className="h-3 w-3" />
            </button>
          </div>
        )}
        <button
          type="button"
          onClick={openAssistant}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={finishDrag}
          onPointerCancel={finishDrag}
          className="flex h-[34px] w-[34px] touch-none select-none items-center justify-center rounded-full border border-violet-500/35 bg-background text-violet-600 shadow-[0_10px_22px_-10px_rgba(109,40,217,0.7)] backdrop-blur transition-[background-color,box-shadow] hover:bg-violet-500/10 active:cursor-grabbing dark:text-violet-300 cursor-grab"
          aria-label={t("codexAssistant.open")}
          title={t("codexAssistant.open")}
        >
          <CodexIcon size={19} className="dark:invert-0" />
        </button>
      </div>

      <aside
        aria-hidden={!open}
        className={cn(
          "fixed inset-y-0 right-0 z-[130] flex w-[390px] max-w-[calc(100vw-24px)] flex-col border-l border-border bg-background shadow-[-18px_0_44px_-24px_rgba(15,23,42,0.45)] transition-transform duration-300 ease-out",
          open ? "translate-x-0" : "translate-x-full",
        )}
      >
        <header className="flex items-center justify-between border-b border-border px-5 py-4">
          <div className="flex items-center gap-3">
            <span className="flex h-9 w-9 items-center justify-center rounded-xl bg-gradient-to-br from-violet-500 to-indigo-500 text-white shadow-sm">
              <CodexIcon size={17} className="dark:invert-0" />
            </span>
            <div>
              <h2 className="text-sm font-semibold">
                {t("codexAssistant.title")}
              </h2>
              <p className="text-xs text-muted-foreground">
                {t("codexAssistant.subtitle")}
              </p>
            </div>
          </div>
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8"
            onPointerDown={(event) => event.stopPropagation()}
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              closeAssistant();
            }}
            aria-label={t("common.close")}
          >
            <X className="h-4 w-4" />
          </Button>
        </header>

        <div className="flex-1 space-y-5 overflow-y-auto p-5">
          <div className="rounded-2xl border border-violet-500/15 bg-violet-500/[0.045] p-4">
            <div className="flex items-start gap-3">
              <span className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-violet-500/10 text-violet-600 dark:text-violet-300">
                <Bot className="h-4 w-4" />
              </span>
              <div className="space-y-1.5 text-sm leading-relaxed">
                <p className="font-medium">{t("codexAssistant.greeting")}</p>
                <p className="text-muted-foreground">
                  {t("codexAssistant.intro")}
                </p>
              </div>
            </div>
          </div>

          <section className="space-y-2">
            <p className="px-1 text-xs font-medium uppercase tracking-[0.12em] text-muted-foreground">
              {t("codexAssistant.readiness")}
            </p>
            <button
              type="button"
              onClick={() => !cliReady && void installCodex()}
              disabled={installing || checking || cliReady}
              className="flex w-full items-center gap-3 rounded-xl border border-border bg-card px-3 py-3 text-left transition hover:border-violet-500/30 disabled:cursor-default"
            >
              <span
                className={cn(
                  "flex h-8 w-8 items-center justify-center rounded-lg",
                  cliReady
                    ? "bg-emerald-500/10 text-emerald-600 dark:text-emerald-300"
                    : "bg-amber-500/10 text-amber-600 dark:text-amber-300",
                )}
              >
                {installing || checking ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : cliReady ? (
                  <Check className="h-4 w-4" />
                ) : (
                  <Terminal className="h-4 w-4" />
                )}
              </span>
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-medium">
                  {t("codexAssistant.codexCli")}
                </span>
                <span className="block text-xs text-muted-foreground">
                  {cliReady
                    ? t("codexAssistant.cliReady", {
                        version: runtime?.version,
                      })
                    : t("codexAssistant.installCli")}
                </span>
              </span>
              {!cliReady && (
                <ChevronRight className="h-4 w-4 text-muted-foreground" />
              )}
            </button>

            <button
              type="button"
              onClick={() => {
                closeAssistant();
                onOpenCodexConfiguration();
              }}
              className="flex w-full items-center gap-3 rounded-xl border border-border bg-card px-3 py-3 text-left transition hover:border-violet-500/30"
            >
              <span
                className={cn(
                  "flex h-8 w-8 items-center justify-center rounded-lg",
                  effectiveProviderReady
                    ? "bg-emerald-500/10 text-emerald-600 dark:text-emerald-300"
                    : "bg-amber-500/10 text-amber-600 dark:text-amber-300",
                )}
              >
                {effectiveProviderReady ? (
                  <Check className="h-4 w-4" />
                ) : (
                  <CircleAlert className="h-4 w-4" />
                )}
              </span>
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-medium">
                  {t("codexAssistant.modelProvider")}
                </span>
                <span className="block text-xs text-muted-foreground">
                  {effectiveProviderReady
                    ? t("codexAssistant.providerReady")
                    : t("codexAssistant.configureProvider")}
                </span>
              </span>
              <ChevronRight className="h-4 w-4 text-muted-foreground" />
            </button>
          </section>

          {installIntent ? (
            <>
              <section className="space-y-2">
                <p className="px-1 text-xs font-medium uppercase tracking-[0.12em] text-muted-foreground">
                  {t("codexAssistant.installVersion")}
                </p>
                <div className="rounded-xl border border-border bg-card p-2">
                  <div className="grid grid-cols-3 gap-1">
                    {(["stable", "latest", ...(supportsVersionPin ? ["specific" as const] : [])] as const).map((mode) => (
                      <Button
                        key={mode}
                        type="button"
                        size="sm"
                        variant={
                          installVersionMode === mode ? "default" : "ghost"
                        }
                        disabled={planning || executing}
                        onClick={() => setInstallVersionMode(mode)}
                      >
                        {t(
                          `codexAssistant.version${mode[0].toUpperCase()}${mode.slice(1)}`,
                        )}
                      </Button>
                    ))}
                  </div>
                  {installVersionMode === "specific" && (
                    <Input
                      value={specificVersion}
                      onChange={(event) =>
                        setSpecificVersion(event.target.value)
                      }
                      disabled={planning || executing}
                      placeholder={t(
                        "codexAssistant.specificVersionPlaceholder",
                      )}
                      className="mt-2 h-8 text-xs"
                    />
                  )}
                </div>
              </section>

              <section className="space-y-2">
                <p className="px-1 text-xs font-medium uppercase tracking-[0.12em] text-muted-foreground">
                  {t("codexAssistant.installLocation")}
                </p>
                <div className="rounded-xl border border-border bg-card p-2">
                  {supportsCustomInstallLocation ? <div className="grid grid-cols-2 gap-1">
                    <Button
                      type="button"
                      size="sm"
                      variant={
                        installLocationMode === "default" ? "default" : "ghost"
                      }
                      disabled={planning || executing}
                      onClick={() => setInstallLocationMode("default")}
                    >
                      {t("codexAssistant.defaultLocation")}
                    </Button>
                    <Button
                      type="button"
                      size="sm"
                      variant={
                        installLocationMode === "custom" ? "default" : "ghost"
                      }
                      disabled={planning || executing}
                      onClick={() => setInstallLocationMode("custom")}
                    >
                      {t("codexAssistant.customLocation")}
                    </Button>
                  </div> : <div className="rounded-lg bg-muted/40 px-3 py-2 text-sm text-muted-foreground">桌面应用由系统安装器管理，将安装到系统默认位置。</div>}
                  <div className="mt-2 flex items-center gap-3 px-1 py-1">
                    <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-sky-500/10 text-sky-600 dark:text-sky-300">
                      <FolderOpen className="h-4 w-4" />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-sm font-medium">
                        {installLocationMode === "default"
                          ? t("codexAssistant.defaultLocationDescription")
                          : targetDirectory ||
                            t("codexAssistant.locationNotSelected")}
                      </span>
                      <span className="block text-xs text-muted-foreground">
                        {installLocationMode === "default"
                          ? t("codexAssistant.defaultLocationHint")
                          : t("codexAssistant.locationHint")}
                      </span>
                    </span>
                    {supportsCustomInstallLocation && installLocationMode === "custom" && (
                      <Button
                        variant="outline"
                        size="sm"
                        className="shrink-0"
                        disabled={planning || executing}
                        onClick={() => void chooseInstallDirectory()}
                      >
                        {t("codexAssistant.chooseLocation")}
                      </Button>
                    )}
                  </div>
                  {webBridgeActive &&
                    supportsCustomInstallLocation &&
                    installLocationMode === "custom" &&
                    webDirectoryEditorOpen && (
                      <div className="mt-2 rounded-lg border border-sky-500/20 bg-sky-500/[0.035] p-2">
                        <p className="mb-1.5 text-[11px] leading-relaxed text-muted-foreground">
                          {t("codexAssistant.webDirectoryHint")}
                        </p>
                        <Input
                          value={webDirectoryDraft}
                          onChange={(event) =>
                            setWebDirectoryDraft(event.target.value)
                          }
                          onKeyDown={(event) => {
                            if (event.key === "Enter") saveWebDirectory();
                          }}
                          placeholder="D:\\AI Tools\\node-global"
                          className="h-8 text-xs"
                        />
                        <div className="mt-2 flex justify-end gap-2">
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            className="h-7 text-xs"
                            onClick={() => setWebDirectoryEditorOpen(false)}
                          >
                            {t("common.cancel")}
                          </Button>
                          <Button
                            type="button"
                            size="sm"
                            className="h-7 text-xs"
                            onClick={saveWebDirectory}
                          >
                            {t("codexAssistant.saveLocation")}
                          </Button>
                        </div>
                      </div>
                    )}
                </div>
              </section>
            </>
          ) : (
            <section className="space-y-2">
              <p className="px-1 text-xs font-medium uppercase tracking-[0.12em] text-muted-foreground">
                {t("codexAssistant.installLocation")}
              </p>
              <div className="flex items-center gap-3 rounded-xl border border-border bg-card p-3">
                <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-sky-500/10 text-sky-600 dark:text-sky-300">
                  <FolderOpen className="h-4 w-4" />
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block text-sm font-medium">
                    {targetDirectory || t("codexAssistant.locationNotSelected")}
                  </span>
                  <span className="block text-xs text-muted-foreground">
                    {t("codexAssistant.locationHint")}
                  </span>
                </span>
                <Button
                  variant="outline"
                  size="sm"
                  className="shrink-0"
                  disabled={planning || executing}
                  onClick={() => void chooseInstallDirectory()}
                >
                  {t("codexAssistant.chooseLocation")}
                </Button>
              </div>
            </section>
          )}

          <section className="space-y-2">
            <p className="px-1 text-xs font-medium uppercase tracking-[0.12em] text-muted-foreground">
              {t("codexAssistant.installPlan")}
            </p>
            {webBridgeActive && (
              <p className="rounded-lg border border-sky-500/20 bg-sky-500/[0.045] px-3 py-2 text-xs leading-relaxed text-sky-800 dark:text-sky-200">
                {t("codexAssistant.webBridgeNotice")}
              </p>
            )}
            <div className="rounded-xl border border-border bg-card p-3">
              <Input
                value={request}
                onChange={(event) => setRequest(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") requestPlan();
                }}
                disabled={!ready || planning || executing}
                placeholder={
                  ready
                    ? t("codexAssistant.requestPlaceholder")
                    : (blockedReason ?? undefined)
                }
                className="border-0 bg-transparent px-0 shadow-none focus:ring-0"
              />
              <div className="mt-2 flex items-center justify-between border-t border-border/70 pt-2">
                <span className="text-[11px] text-muted-foreground">
                  {ready ? t("codexAssistant.planFirst") : blockedReason}
                </span>
                <Button
                  size="icon"
                  className="h-8 w-8 rounded-lg"
                  disabled={!ready || !request.trim() || planning || executing}
                  onClick={() => void requestPlan()}
                  aria-label={t("codexAssistant.createPlan")}
                >
                  {planning ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <Send className="h-3.5 w-3.5" />
                  )}
                </Button>
              </div>
            </div>

            {(planning || executing) && activeRunId && (
              <Button
                variant="outline"
                size="sm"
                className="w-full"
                disabled={stopping}
                onClick={() => void cancelRun()}
              >
                {stopping ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <CircleStop className="h-3.5 w-3.5" />
                )}
                {stopping
                  ? t("codexAssistant.cancelling")
                  : t("codexAssistant.stopRun")}
              </Button>
            )}

            {runError && (
              <p className="rounded-lg border border-destructive/25 bg-destructive/5 px-3 py-2 text-xs leading-relaxed text-destructive">
                {runError}
              </p>
            )}

            {plan && planId && (
              <div className="space-y-3 rounded-xl border border-violet-500/20 bg-violet-500/[0.035] p-3">
                <div>
                  <p className="text-sm font-semibold">{plan.title}</p>
                  <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                    {plan.summary}
                  </p>
                </div>
                {plan.sources.length > 0 && (
                  <div>
                    <p className="text-[11px] font-medium uppercase tracking-[0.1em] text-muted-foreground">
                      {t("codexAssistant.sources")}
                    </p>
                    <ul className="mt-1.5 space-y-1 text-xs text-muted-foreground">
                      {plan.sources.map((source) => (
                        <li key={source} className="break-all">
                          • {source}
                        </li>
                      ))}
                    </ul>
                  </div>
                )}
                {plan.install && (
                  <div className="rounded-lg border border-violet-500/15 bg-background/55 px-2.5 py-2 text-xs leading-relaxed">
                    <p className="font-medium text-foreground">
                      {plan.install.displayName} · {plan.install.version}
                    </p>
                    <p className="mt-0.5 break-all text-muted-foreground">
                      {t("codexAssistant.installingTo", {
                        location: plan.install.usesDefaultLocation
                          ? t("codexAssistant.defaultLocationDescription")
                          : plan.install.installLocation,
                      })}
                    </p>
                    <p className="mt-0.5 break-all text-muted-foreground">
                      {t("codexAssistant.registeredSource", {
                        source: plan.install.officialSource,
                      })}
                    </p>
                  </div>
                )}
                {plan.steps.length > 0 && (
                  <ol className="space-y-2">
                    {plan.steps.map((step, index) => (
                      <li
                        key={`${step.label}-${index}`}
                        className="flex gap-2 text-xs"
                      >
                        <span className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-violet-500/10 text-[10px] font-medium text-violet-700 dark:text-violet-200">
                          {index + 1}
                        </span>
                        <span>
                          <span className="font-medium text-foreground">
                            {step.label}
                          </span>
                          {step.requiresNetwork && (
                            <span className="ml-1 text-muted-foreground">
                              · {t("codexAssistant.networkRequired")}
                            </span>
                          )}
                          <span className="block leading-relaxed text-muted-foreground">
                            {step.description}
                          </span>
                        </span>
                      </li>
                    ))}
                  </ol>
                )}
                {plan.limitations.length > 0 && (
                  <div className="rounded-lg bg-amber-500/10 px-2.5 py-2 text-xs text-amber-800 dark:text-amber-200">
                    <p className="font-medium">
                      {t("codexAssistant.limitations")}
                    </p>
                    <ul className="mt-1 space-y-1">
                      {plan.limitations.map((limitation) => (
                        <li key={limitation}>• {limitation}</li>
                      ))}
                    </ul>
                  </div>
                )}
                {plan.executable ? (
                  <Button
                    className="w-full"
                    disabled={planning || executing}
                    onClick={() => void executePlan()}
                  >
                    {executing ? (
                      <Loader2 className="h-4 w-4 animate-spin" />
                    ) : (
                      <Check className="h-4 w-4" />
                    )}
                    {executing
                      ? t("codexAssistant.installingPlan")
                      : t("codexAssistant.confirmInstall")}
                  </Button>
                ) : (
                  <p className="rounded-lg bg-muted/60 px-2.5 py-2 text-xs leading-relaxed text-muted-foreground">
                    {t("codexAssistant.manualOnly")}
                  </p>
                )}
              </div>
            )}

            {logs.length > 0 && (
              <div className="overflow-hidden rounded-xl border border-border bg-muted/35">
                <p className="border-b border-border px-3 py-2 text-[11px] font-medium uppercase tracking-[0.1em] text-muted-foreground">
                  {t("codexAssistant.executionLog")}
                </p>
                <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-words px-3 py-2 font-mono text-[11px] leading-relaxed text-muted-foreground">
                  {logs.join("\n")}
                </pre>
              </div>
            )}
          </section>
        </div>

        <footer className="border-t border-border px-5 py-3 text-xs text-muted-foreground">
          <span className="inline-flex items-center gap-1.5">
            <MessageCircle className="h-3.5 w-3.5 text-violet-500" />
            {t("codexAssistant.safetyNote")}
          </span>
        </footer>
      </aside>
    </>
  );
}
