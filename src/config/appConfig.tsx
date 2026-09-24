import React from "react";
import type { AppId } from "@/lib/api/types";
import type { VisibleApps } from "@/types";
import { AgentIcon, getAgentVisual } from "@/components/AgentIcon";

export interface AppConfig {
  label: string;
  icon: React.ReactNode;
  activeClass: string;
  badgeClass: string;
}

export const APP_IDS: AppId[] = [
  "claude",
  "claude-desktop",
  "codex",
  "gemini",
  "grokbuild",
  "opencode",
  "openclaw",
  "hermes",
  "pi",
  "qoder",
  "dsh",
];

export const DEFAULT_VISIBLE_APPS: VisibleApps = {
  claude: true,
  "claude-desktop": true,
  codex: true,
  gemini: true,
  grokbuild: true,
  opencode: true,
  openclaw: true,
  hermes: true,
  pi: true,
  qoder: true,
  dsh: true,
};

/** App IDs shown in Skills panels. */
export const SKILLS_APP_IDS: AppId[] = [
  "claude",
  "codex",
  "gemini",
  "grokbuild",
  "opencode",
  "hermes",
  "pi",
];

export type ProxyAppId = Extract<
  AppId,
  "claude" | "codex" | "gemini" | "grokbuild"
>;

/** Apps with a complete local gateway + failover data plane. */
export const PROXY_APP_IDS: ProxyAppId[] = [
  "claude",
  "codex",
  "gemini",
  "grokbuild",
];

export function isProxyAppId(appId: string): appId is ProxyAppId {
  return (PROXY_APP_IDS as string[]).includes(appId);
}

export type ProviderMode = "switch" | "additive";

export const PROVIDER_MODE_BY_APP: Record<AppId, ProviderMode> = {
  claude: "switch",
  "claude-desktop": "switch",
  codex: "switch",
  gemini: "switch",
  grokbuild: "switch",
  opencode: "additive",
  openclaw: "additive",
  hermes: "additive",
  pi: "additive",
  qoder: "additive",
  dsh: "additive",
};

export type AdditiveAppId = Extract<
  AppId,
  "opencode" | "openclaw" | "hermes" | "pi" | "qoder" | "dsh"
>;

export const ADDITIVE_APP_IDS = APP_IDS.filter(
  (appId): appId is AdditiveAppId => PROVIDER_MODE_BY_APP[appId] === "additive",
);

export function isAdditiveAppId(appId: string): appId is AdditiveAppId {
  return PROVIDER_MODE_BY_APP[appId as AppId] === "additive";
}

/** Pi has no native MCP registry; do not manufacture a disabled mirror.
 *  DSH has no MCP support either. */
export type McpAppId = Exclude<
  AppId,
  "claude-desktop" | "openclaw" | "pi" | "qoder" | "dsh"
>;
export const MCP_APP_IDS: McpAppId[] = [
  "claude",
  "codex",
  "gemini",
  "grokbuild",
  "opencode",
  "hermes",
];

export function isMcpAppId(appId: string): appId is McpAppId {
  return (MCP_APP_IDS as string[]).includes(appId);
}

export const APP_ICON_MAP: Record<AppId, AppConfig> = {
  claude: {
    label: getAgentVisual("claude").shortLabel,
    icon: <AgentIcon agentId="claude" size={14} />,
    activeClass:
      "bg-orange-500/10 ring-1 ring-orange-500/20 hover:bg-orange-500/20 text-orange-600 dark:text-orange-400",
    badgeClass:
      "bg-orange-500/10 text-orange-700 dark:text-orange-300 hover:bg-orange-500/20 border-0 gap-1.5",
  },
  "claude-desktop": {
    label: getAgentVisual("claude-desktop").shortLabel,
    icon: <AgentIcon agentId="claude-desktop" size={14} />,
    activeClass:
      "bg-amber-500/10 ring-1 ring-amber-500/20 hover:bg-amber-500/20 text-amber-700 dark:text-amber-300",
    badgeClass:
      "bg-amber-500/10 text-amber-700 dark:text-amber-300 hover:bg-amber-500/20 border-0 gap-1.5",
  },
  codex: {
    label: getAgentVisual("codex").shortLabel,
    icon: <AgentIcon agentId="codex" size={14} />,
    activeClass:
      "bg-green-500/10 ring-1 ring-green-500/20 hover:bg-green-500/20 text-green-600 dark:text-green-400",
    badgeClass:
      "bg-green-500/10 text-green-700 dark:text-green-300 hover:bg-green-500/20 border-0 gap-1.5",
  },
  gemini: {
    label: getAgentVisual("gemini").shortLabel,
    icon: <AgentIcon agentId="gemini" size={14} />,
    activeClass:
      "bg-blue-500/10 ring-1 ring-blue-500/20 hover:bg-blue-500/20 text-blue-600 dark:text-blue-400",
    badgeClass:
      "bg-blue-500/10 text-blue-700 dark:text-blue-300 hover:bg-blue-500/20 border-0 gap-1.5",
  },
  grokbuild: {
    label: getAgentVisual("grokbuild").shortLabel,
    icon: <AgentIcon agentId="grokbuild" size={14} showFallback={false} />,
    activeClass:
      "bg-cyan-500/10 ring-1 ring-cyan-500/20 hover:bg-cyan-500/20 text-cyan-700 dark:text-cyan-300",
    badgeClass:
      "bg-cyan-500/10 text-cyan-700 dark:text-cyan-300 hover:bg-cyan-500/20 border-0 gap-1.5",
  },
  opencode: {
    label: getAgentVisual("opencode").shortLabel,
    icon: <AgentIcon agentId="opencode" size={14} showFallback={false} />,
    activeClass:
      "bg-indigo-500/10 ring-1 ring-indigo-500/20 hover:bg-indigo-500/20 text-indigo-600 dark:text-indigo-400",
    badgeClass:
      "bg-indigo-500/10 text-indigo-700 dark:text-indigo-300 hover:bg-indigo-500/20 border-0 gap-1.5",
  },
  openclaw: {
    label: getAgentVisual("openclaw").shortLabel,
    icon: <AgentIcon agentId="openclaw" size={14} />,
    activeClass:
      "bg-rose-500/10 ring-1 ring-rose-500/20 hover:bg-rose-500/20 text-rose-600 dark:text-rose-400",
    badgeClass:
      "bg-rose-500/10 text-rose-700 dark:text-rose-300 hover:bg-rose-500/20 border-0 gap-1.5",
  },
  hermes: {
    label: getAgentVisual("hermes").shortLabel,
    icon: <AgentIcon agentId="hermes" size={14} showFallback={false} />,
    activeClass:
      "bg-violet-500/10 ring-1 ring-violet-500/20 hover:bg-violet-500/20 text-violet-600 dark:text-violet-400",
    badgeClass:
      "bg-violet-500/10 text-violet-700 dark:text-violet-300 hover:bg-violet-500/20 border-0 gap-1.5",
  },
  pi: {
    label: getAgentVisual("pi").shortLabel,
    icon: <AgentIcon agentId="pi" size={14} showFallback={false} />,
    activeClass:
      "bg-fuchsia-500/10 ring-1 ring-fuchsia-500/20 hover:bg-fuchsia-500/20 text-fuchsia-600 dark:text-fuchsia-400",
    badgeClass:
      "bg-fuchsia-500/10 text-fuchsia-700 dark:text-fuchsia-300 hover:bg-fuchsia-500/20 border-0 gap-1.5",
  },
  qoder: {
    label: getAgentVisual("qoder").shortLabel,
    icon: <AgentIcon agentId="qoder" size={14} showFallback={false} />,
    activeClass:
      "bg-blue-500/10 ring-1 ring-blue-500/20 hover:bg-blue-500/20 text-blue-600 dark:text-blue-400",
    badgeClass:
      "bg-blue-500/10 text-blue-700 dark:text-blue-300 hover:bg-blue-500/20 border-0 gap-1.5",
  },
  dsh: {
    label: getAgentVisual("dsh").label,
    icon: <AgentIcon agentId="dsh" size={14} showFallback={false} />,
    activeClass:
      "bg-blue-500/10 ring-1 ring-blue-500/20 hover:bg-blue-500/20 text-blue-600 dark:text-blue-400",
    badgeClass:
      "bg-blue-500/10 text-blue-700 dark:text-blue-300 hover:bg-blue-500/20 border-0 gap-1.5",
  },
};

export function getAppLabel(appId: string): string {
  return APP_ICON_MAP[appId as AppId]?.label ?? appId;
}
