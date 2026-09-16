import { useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import {
  Bot,
  Check,
  ChevronRight,
  CircleAlert,
  CircleStop,
  FilePenLine,
  Loader2,
  MessageCircle,
  Send,
  ShieldAlert,
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
  type CodexAssistantApproval,
  type CodexAssistantApprovalDecision,
  type CodexAssistantChatTurn,
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

const FLOATING_BUTTON_SIZE = 34;
const FLOATING_BUTTON_MARGIN = 16;
const FLOATING_POSITION_STORAGE_KEY = "cc-switch-codex-assistant-position";
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

function ApprovalCard({
  approval,
  responding,
  onRespond,
}: {
  approval: CodexAssistantApproval;
  responding: boolean;
  onRespond: (decision: CodexAssistantApprovalDecision) => void;
}) {
  const { t } = useTranslation();
  const isCommand = approval.type === "command";
  const supports = (decision: CodexAssistantApprovalDecision) =>
    approval.availableDecisions.includes(decision);

  return (
    <div
      className="space-y-3 rounded-xl border border-amber-500/30 bg-amber-500/[0.055] p-3"
      data-testid={`codex-approval-${approval.id}`}
    >
      <div className="flex items-start gap-2.5">
        <span className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-amber-500/15 text-amber-700 dark:text-amber-200">
          {isCommand ? (
            <Terminal className="h-3.5 w-3.5" />
          ) : (
            <FilePenLine className="h-3.5 w-3.5" />
          )}
        </span>
        <div className="min-w-0">
          <p className="text-sm font-semibold">
            {isCommand
              ? t("codexAssistant.commandApproval", {
                  defaultValue: "Command approval",
                })
              : t("codexAssistant.fileApproval", {
                  defaultValue: "File change approval",
                })}
          </p>
          {approval.reason && (
            <p className="mt-0.5 text-xs leading-relaxed text-muted-foreground">
              {approval.reason}
            </p>
          )}
        </div>
      </div>

      {approval.command && (
        <pre className="max-h-36 overflow-auto whitespace-pre-wrap break-all rounded-lg border border-border/70 bg-background/70 px-2.5 py-2 font-mono text-[11px] leading-relaxed">
          {approval.command}
        </pre>
      )}

      <dl className="space-y-1 text-[11px] text-muted-foreground">
        {approval.cwd && (
          <div className="flex gap-2">
            <dt className="shrink-0 font-medium text-foreground/80">
              {t("codexAssistant.workingDirectory", {
                defaultValue: "Working directory",
              })}
            </dt>
            <dd className="min-w-0 break-all">{approval.cwd}</dd>
          </div>
        )}
        {approval.networkHost && (
          <div className="flex gap-2">
            <dt className="shrink-0 font-medium text-foreground/80">
              {t("codexAssistant.networkHost", { defaultValue: "Network" })}
            </dt>
            <dd className="min-w-0 break-all">{approval.networkHost}</dd>
          </div>
        )}
        {approval.grantRoot && (
          <div className="flex gap-2">
            <dt className="shrink-0 font-medium text-foreground/80">
              {t("codexAssistant.grantRoot", { defaultValue: "Write access" })}
            </dt>
            <dd className="min-w-0 break-all">{approval.grantRoot}</dd>
          </div>
        )}
      </dl>

      <div className="flex flex-wrap justify-end gap-2">
        {supports("cancel") && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={responding}
            onClick={() => onRespond("cancel")}
          >
            {t("codexAssistant.cancelApproval", {
              defaultValue: "Cancel task",
            })}
          </Button>
        )}
        {supports("decline") && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={responding}
            onClick={() => onRespond("decline")}
          >
            {t("codexAssistant.declineApproval", { defaultValue: "Deny" })}
          </Button>
        )}
        {supports("accept") && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={responding}
            onClick={() => onRespond("accept")}
          >
            {t("codexAssistant.approveOnce", { defaultValue: "Allow once" })}
          </Button>
        )}
        {approval.allowForSession && supports("acceptForSession") && (
          <Button
            type="button"
            size="sm"
            disabled={responding}
            onClick={() => onRespond("acceptForSession")}
          >
            {responding && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
            {t("codexAssistant.approveForSession", {
              defaultValue: "Allow for session",
            })}
          </Button>
        )}
      </div>
    </div>
  );
}

/** A persistent Codex app-server conversation with explicit approval prompts. */
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
  const [chatMessages, setChatMessages] = useState<CodexAssistantChatTurn[]>(
    [],
  );
  const [runtime, setRuntime] = useState<CodexRuntime | null>(null);
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [request, setRequest] = useState("");
  const [logs, setLogs] = useState<string[]>([]);
  const [approvals, setApprovals] = useState<CodexAssistantApproval[]>([]);
  const [respondingApprovalIds, setRespondingApprovalIds] = useState<
    Set<string>
  >(new Set());
  const [runError, setRunError] = useState<string | null>(null);
  const [startingSession, setStartingSession] = useState(false);
  const [running, setRunning] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [sessionId, setSessionId] = useState<string | null>(null);
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
  const sessionIdRef = useRef<string | null>(null);
  const streamAssistantMessageRef = useRef(false);
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
        // Ignore stale stored coordinates.
      }
    }
    const next = defaultFloatingPosition();
    latestFloatingPositionRef.current = next;
    setFloatingPosition(next);
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
      const activeSessionId = sessionIdRef.current;
      sessionIdRef.current = null;
      if (activeSessionId) {
        void settingsApi.closeCodexAssistantSession(activeSessionId);
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
        if (event.sessionId !== sessionIdRef.current) return;

        switch (event.kind) {
          case "started":
            streamAssistantMessageRef.current = false;
            setRunning(true);
            return;
          case "log":
            if (event.message) appendLog(event.message);
            return;
          case "stderr":
            if (event.message) appendLog(`stderr · ${event.message}`);
            return;
          case "disconnected":
            sessionIdRef.current = null;
            setSessionId(null);
            streamAssistantMessageRef.current = false;
            setRunning(false);
            setStopping(false);
            setApprovals([]);
            setRespondingApprovalIds(new Set());
            setRunError(event.message);
            toast.error(t("codexAssistant.runFailed"), {
              description: event.message,
            });
            return;
          case "message":
            if (event.message) {
              setChatMessages((current) => {
                if (streamAssistantMessageRef.current) {
                  const last = current.at(-1);
                  if (last?.role === "assistant") {
                    return [
                      ...current.slice(0, -1),
                      { ...last, content: last.content + event.message },
                    ];
                  }
                }
                streamAssistantMessageRef.current = true;
                return [
                  ...current,
                  { role: "assistant", content: event.message },
                ];
              });
            }
            return;
          case "approval":
            setApprovals((current) => [
              ...current.filter((item) => item.id !== event.approval.id),
              event.approval,
            ]);
            return;
          case "finished":
            streamAssistantMessageRef.current = false;
            setRunning(false);
            setStopping(false);
            setApprovals([]);
            setRespondingApprovalIds(new Set());
            if (!event.success && !event.cancelled) {
              const message = event.message || t("codexAssistant.runFailed");
              setRunError(message);
              toast.error(t("codexAssistant.runFailed"), {
                description: message,
              });
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
  }, [appendLog, t]);

  useEffect(() => {
    if (open) void refreshRuntime();
  }, [open, refreshRuntime]);

  const installCodex = async () => {
    setInstalling(true);
    try {
      await settingsApi.runToolLifecycleAction(["codex"], "install");
      await refreshRuntime();
      onInstallationCompleted?.("codex");
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
  const effectiveCliReady = cliReady || webBridgeActive;
  const effectiveProviderReady = providerReady || webBridgeActive;
  const ready = effectiveCliReady && effectiveProviderReady;
  const blockedReason = !effectiveCliReady
    ? t("codexAssistant.cliRequired")
    : !effectiveProviderReady
      ? t("codexAssistant.providerRequired")
      : null;

  const ensureSession = async (): Promise<string> => {
    if (sessionIdRef.current) return sessionIdRef.current;
    setStartingSession(true);
    try {
      const nextSessionId = await settingsApi.startCodexAssistantSession();
      sessionIdRef.current = nextSessionId;
      setSessionId(nextSessionId);
      return nextSessionId;
    } finally {
      setStartingSession(false);
    }
  };

  const sendMessage = async () => {
    const text = request.trim();
    if (!text || running || startingSession) return;

    setRequest("");
    setRunError(null);
    setLogs([]);
    streamAssistantMessageRef.current = false;
    setChatMessages((current) => [...current, { role: "user", content: text }]);
    try {
      const activeSessionId = await ensureSession();
      setRunning(true);
      await settingsApi.sendCodexAssistantMessage(activeSessionId, text);
    } catch (error) {
      setRunning(false);
      const message =
        extractErrorMessage(error) || t("codexAssistant.runFailed");
      setRunError(message);
      toast.error(t("codexAssistant.runFailed"), { description: message });
    }
  };

  const respondToApproval = async (
    approvalId: string,
    decision: CodexAssistantApprovalDecision,
  ) => {
    const activeSessionId = sessionIdRef.current;
    if (!activeSessionId || respondingApprovalIds.has(approvalId)) return;
    setRespondingApprovalIds((current) => new Set(current).add(approvalId));
    try {
      await settingsApi.respondCodexAssistantApproval(
        activeSessionId,
        approvalId,
        decision,
      );
      setApprovals((current) =>
        current.filter((approval) => approval.id !== approvalId),
      );
    } catch (error) {
      toast.error(
        t("codexAssistant.approvalFailed", {
          defaultValue: "Could not send the approval decision",
        }),
        { description: extractErrorMessage(error) || undefined },
      );
    } finally {
      setRespondingApprovalIds((current) => {
        const next = new Set(current);
        next.delete(approvalId);
        return next;
      });
    }
  };

  const cancelRun = async () => {
    const activeSessionId = sessionIdRef.current;
    if (!activeSessionId || stopping) return;
    setStopping(true);
    try {
      await settingsApi.cancelCodexAssistantRun(activeSessionId);
      appendLog(t("codexAssistant.cancelling"));
    } catch (error) {
      setStopping(false);
      toast.error(t("codexAssistant.cancelFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    }
  };

  const closeSession = () => {
    const activeSessionId = sessionIdRef.current;
    sessionIdRef.current = null;
    setSessionId(null);
    streamAssistantMessageRef.current = false;
    setRunning(false);
    setStopping(false);
    setApprovals([]);
    setRespondingApprovalIds(new Set());
    if (activeSessionId) {
      void settingsApi
        .closeCodexAssistantSession(activeSessionId)
        .catch(() => undefined);
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
    closeSession();
    onDismiss?.();
  };

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
          className="flex h-[34px] w-[34px] cursor-grab touch-none select-none items-center justify-center rounded-full border border-violet-500/35 bg-background text-violet-600 shadow-[0_10px_22px_-10px_rgba(109,40,217,0.7)] backdrop-blur transition-[background-color,box-shadow] hover:bg-violet-500/10 active:cursor-grabbing dark:text-violet-300"
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
            onClick={closeAssistant}
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

          <section className="space-y-3">
            <p className="px-1 text-xs font-medium uppercase tracking-[0.12em] text-muted-foreground">
              {t("codexAssistant.chatLabel")}
            </p>
            {webBridgeActive && (
              <p className="rounded-lg border border-sky-500/20 bg-sky-500/[0.045] px-3 py-2 text-xs leading-relaxed text-sky-800 dark:text-sky-200">
                {t("codexAssistant.webBridgeNotice")}
              </p>
            )}

            <div className="space-y-2" aria-live="polite">
              {chatMessages.length === 0 ? (
                <p className="rounded-lg border border-border/70 bg-muted/30 px-3 py-2 text-xs leading-relaxed text-muted-foreground">
                  {t("codexAssistant.chatEmpty")}
                </p>
              ) : (
                chatMessages.map((message, index) => (
                  <div
                    key={`${message.role}-${index}`}
                    className={cn(
                      "max-w-[92%] whitespace-pre-wrap break-words rounded-xl px-3 py-2 text-sm leading-relaxed",
                      message.role === "user"
                        ? "ml-auto bg-violet-500/15 text-foreground"
                        : "mr-auto border border-border bg-card",
                    )}
                  >
                    {message.content}
                  </div>
                ))
              )}
            </div>

            {approvals.length > 0 && (
              <div className="space-y-2" aria-live="assertive">
                <p className="flex items-center gap-1.5 px-1 text-xs font-medium text-amber-700 dark:text-amber-200">
                  <ShieldAlert className="h-3.5 w-3.5" />
                  {t("codexAssistant.approvalRequired", {
                    defaultValue: "Your approval is required",
                  })}
                </p>
                {approvals.map((approval) => (
                  <ApprovalCard
                    key={approval.id}
                    approval={approval}
                    responding={respondingApprovalIds.has(approval.id)}
                    onRespond={(decision) =>
                      void respondToApproval(approval.id, decision)
                    }
                  />
                ))}
              </div>
            )}

            <div className="rounded-xl border border-border bg-card p-3">
              <Input
                value={request}
                onChange={(event) => setRequest(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void sendMessage();
                }}
                disabled={!ready || running || startingSession}
                placeholder={
                  ready
                    ? t("codexAssistant.chatPlaceholder")
                    : (blockedReason ?? undefined)
                }
                className="border-0 bg-transparent px-0 shadow-none focus:ring-0"
              />
              <div className="mt-2 flex items-center justify-between border-t border-border/70 pt-2">
                <span className="text-[11px] text-muted-foreground">
                  {ready ? t("codexAssistant.chatHint") : blockedReason}
                </span>
                <Button
                  size="icon"
                  className="h-8 w-8 rounded-lg"
                  disabled={
                    !ready || !request.trim() || running || startingSession
                  }
                  onClick={() => void sendMessage()}
                  aria-label={t("codexAssistant.sendChat")}
                >
                  {running || startingSession ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <Send className="h-3.5 w-3.5" />
                  )}
                </Button>
              </div>
            </div>

            {running && sessionId && (
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
