import { useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AppId } from "@/lib/api";
import type { VisibleApps } from "@/types";
import {
  AgentIcon,
  getAgentVisual,
  useAgentVisualRegistryRevision,
} from "@/components/AgentIcon";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import { Monitor, MoreHorizontal, Plus, Terminal } from "lucide-react";
import { APP_IDS } from "@/config/appConfig";
import type { ManagedAgent } from "@/lib/managedAgents";

const APP_BADGE_ICON: Partial<
  Record<AppId, { icon: typeof Terminal; offsetY?: number }>
> = {
  claude: { icon: Terminal },
  "claude-desktop": { icon: Monitor, offsetY: 0.5 },
};

interface AppSwitcherProps {
  activeApp: AppId;
  onSwitch: (app: AppId) => void;
  managedAgents?: ManagedAgent[];
  activeManagedAgentId?: string | null;
  onSwitchManagedAgent?: (agentId: string) => void;
  visibleApps?: VisibleApps;
  /** Opens the controlled Codex-assisted custom-Agent flow. */
  onAddCustomAgent?: () => void;
}

const STORAGE_KEY = "cc-switch-last-app";

/** 应用图标 + 角标（Claude Code / Desktop 用角标区分终端与桌面） */
function AppGlyph({ app, isActive }: { app: string; isActive: boolean }) {
  const badgeConfig = APP_BADGE_ICON[app as AppId];
  const BadgeIcon = badgeConfig?.icon;
  return (
    <span className="relative inline-flex shrink-0">
      <AgentIcon agentId={app} size={20} />
      {BadgeIcon && (
        <span
          className={cn(
            "absolute -bottom-0.5 -right-0.5 flex items-center justify-center rounded-[3px] border h-[11px] w-[11px]",
            isActive
              ? "bg-background border-border text-foreground"
              : "bg-muted border-background text-muted-foreground group-hover:bg-background group-hover:text-foreground",
          )}
          aria-hidden="true"
        >
          <BadgeIcon
            className="h-[8px] w-[8px]"
            strokeWidth={2.5}
            style={
              badgeConfig?.offsetY
                ? { transform: `translateY(${badgeConfig.offsetY}px)` }
                : undefined
            }
          />
        </span>
      )}
    </span>
  );
}

export function AppSwitcher({
  activeApp,
  onSwitch,
  managedAgents = [],
  activeManagedAgentId,
  onSwitchManagedAgent,
  visibleApps,
  onAddCustomAgent,
}: AppSwitcherProps) {
  const { t } = useTranslation();
  useAgentVisualRegistryRevision();
  const rootRef = useRef<HTMLDivElement>(null);
  const [moreOpen, setMoreOpen] = useState(false);

  const managedAgentIds = new Set(managedAgents.map((agent) => agent.id));
  const selectedAgentId = activeManagedAgentId ?? activeApp;

  const handleSwitch = (app: string) => {
    if (app === selectedAgentId) return;
    if (managedAgentIds.has(app)) {
      localStorage.setItem(STORAGE_KEY, `managed:${app}`);
      onSwitchManagedAgent?.(app);
      return;
    }
    localStorage.setItem(STORAGE_KEY, app);
    onSwitch(app as AppId);
  };

  // Filter apps based on visibility settings (default all visible)
  const builtinApps = APP_IDS.filter((app) => {
    if (!visibleApps) return true;
    return visibleApps[app];
  });
  const appsToShow = [
    ...builtinApps,
    ...managedAgents
      .map((agent) => agent.id)
      .filter((id) => !APP_IDS.includes(id as AppId)),
  ];
  const appCount = appsToShow.length;

  const [visibleCount, setVisibleCount] = useState(appCount);

  // 宽度必须取父弹性槽而非自身：自身宽度随可见数量变化，
  // 用它做输入会形成收起→变窄→再收起的反馈循环
  useLayoutEffect(() => {
    const root = rootRef.current;
    const slot = root?.parentElement;
    if (!root || !slot) return;

    const compute = () => {
      const sample = root.querySelector("button");
      if (!sample) return;
      const itemWidth = sample.offsetWidth;
      // jsdom 或未完成布局时 offsetWidth 为 0，保持全部可见
      if (itemWidth <= 0) return;
      const rootStyle = window.getComputedStyle(root);
      const gap = parseFloat(rootStyle.columnGap) || 0;
      const padding =
        (parseFloat(rootStyle.paddingLeft) || 0) +
        (parseFloat(rootStyle.paddingRight) || 0);
      const available = slot.clientWidth;
      // 始终为“更多 / 添加自定义 Agent”保留一个槽位。即使所有内置 Agent 都放得下，
      // 用户仍然能从同一入口找到添加动作，而不是只能在窗口变窄时才出现。
      const widthAll =
        padding + appCount * itemWidth + (appCount - 1) * gap + itemWidth + gap;
      if (widthAll <= available) {
        setVisibleCount(appCount);
        return;
      }
      // 「更多」按钮与应用按钮同宽（同 padding + 同尺寸图标）
      const fit = Math.floor(
        (available - padding - itemWidth) / (itemWidth + gap),
      );
      setVisibleCount(Math.max(1, Math.min(appCount - 1, fit)));
    };

    compute();
    const observer = new ResizeObserver(compute);
    observer.observe(slot);
    return () => observer.disconnect();
  }, [appCount]);

  const visibleList = appsToShow.slice(0, Math.max(1, visibleCount));
  // 激活应用被收进溢出区时，顶替最后一个可见位，保证始终可点亮
  if (
    appsToShow.includes(selectedAgentId) &&
    !visibleList.includes(selectedAgentId)
  ) {
    visibleList[visibleList.length - 1] = selectedAgentId;
  }
  const overflowList = appsToShow.filter((app) => !visibleList.includes(app));

  return (
    <div
      ref={rootRef}
      className="inline-flex bg-muted rounded-xl p-1 gap-1"
      style={{ WebkitAppRegion: "no-drag" } as any}
    >
      {visibleList.map((app) => {
        const isActive = selectedAgentId === app;
        return (
          <button
            key={app}
            type="button"
            onClick={() => handleSwitch(app)}
            title={getAgentVisual(app).shortLabel}
            aria-label={getAgentVisual(app).shortLabel}
            className={cn(
              "group inline-flex items-center px-3 h-8 rounded-md text-sm font-medium transition-all duration-200",
              isActive
                ? "bg-background text-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground hover:bg-background/50",
            )}
          >
            <AppGlyph app={app} isActive={isActive} />
          </button>
        );
      })}
      <Popover open={moreOpen} onOpenChange={setMoreOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            title={t("appSwitcher.more")}
            aria-label={t("appSwitcher.more")}
            className={cn(
              "inline-flex items-center px-3 h-8 rounded-md transition-all duration-200",
              moreOpen
                ? "bg-background text-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground hover:bg-background/50",
            )}
          >
            <MoreHorizontal size={20} className="shrink-0" />
          </button>
        </PopoverTrigger>
        <PopoverContent
          side="bottom"
          align="end"
          sideOffset={6}
          className="z-[100] w-56 p-1"
        >
          {overflowList.map((app) => (
            <button
              key={app}
              type="button"
              onClick={() => {
                setMoreOpen(false);
                handleSwitch(app);
              }}
              className="group flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            >
              <AppGlyph app={app} isActive={false} />
              <span className="truncate">{getAgentVisual(app).shortLabel}</span>
            </button>
          ))}
          <div
            className={cn(
              "mt-1 border-t border-border/70 pt-1",
              overflowList.length === 0 && "mt-0 border-t-0 pt-0",
            )}
          >
            <button
              type="button"
              onClick={() => {
                setMoreOpen(false);
                onAddCustomAgent?.();
              }}
              className="group flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-sm font-medium text-primary transition-colors hover:bg-primary/10"
            >
              <span className="flex h-5 w-5 items-center justify-center rounded-md border border-primary/30 bg-primary/10">
                <Plus className="h-3.5 w-3.5" strokeWidth={2.5} />
              </span>
              <span className="min-w-0">
                <span className="block">{t("appSwitcher.addCustomAgent")}</span>
                <span className="block truncate text-[11px] font-normal text-muted-foreground group-hover:text-primary/70">
                  {t("appSwitcher.addCustomAgentHint")}
                </span>
              </span>
            </button>
          </div>
        </PopoverContent>
      </Popover>
    </div>
  );
}
