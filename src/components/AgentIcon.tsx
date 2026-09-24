import { useSyncExternalStore } from "react";
import type { AppId } from "@/lib/api/types";
import { ProviderIcon } from "@/components/ProviderIcon";
import { cn } from "@/lib/utils";

export interface AgentVisual {
  label: string;
  shortLabel?: string;
  /** Provider icon key or an image URL supplied by an installed Agent. */
  icon?: string;
  color?: string;
  iconClassName?: string;
}

const BUILTIN_AGENT_VISUALS = {
  claude: { label: "Claude Code", shortLabel: "Claude", icon: "claude" },
  "claude-desktop": { label: "Claude Desktop", icon: "claude" },
  codex: { label: "Codex", icon: "openai" },
  gemini: { label: "Gemini", icon: "gemini" },
  grokbuild: { label: "Grok Build", icon: "grok" },
  opencode: { label: "OpenCode", icon: "opencode" },
  openclaw: { label: "OpenClaw", icon: "openclaw" },
  hermes: { label: "Hermes", icon: "hermes" },
  pi: { label: "Pi", icon: "pi" },
  dsh: { label: "DeepSeek Harness", shortLabel: "DSH", icon: "dsh" },
} satisfies Record<AppId, AgentVisual>;

const MANAGED_AGENT_VISUALS: Record<string, AgentVisual> = {
  qoder: { label: "Qoder", icon: "qoder" },
};

const overrides = new Map<string, Partial<AgentVisual>>();
const listeners = new Set<() => void>();
let revision = 0;

function normalizeAgentId(agentId: string): string {
  return agentId.trim().toLowerCase();
}

function labelFromId(agentId: string): string {
  return agentId
    .split(/[-_\s]+/)
    .filter(Boolean)
    .map((part) => part[0]?.toUpperCase() + part.slice(1))
    .join(" ");
}

function notifyVisualChange() {
  revision += 1;
  listeners.forEach((listener) => listener());
}

/**
 * Resolve visuals by Agent ID. A matching icon key is picked automatically;
 * installers can register a catalog key or image URL to replace it at runtime.
 */
export function getAgentVisual(
  agentId: string,
): Required<Pick<AgentVisual, "label" | "shortLabel" | "icon">> &
  Pick<AgentVisual, "color" | "iconClassName"> {
  const id = normalizeAgentId(agentId);
  const builtin =
    (BUILTIN_AGENT_VISUALS as Record<string, AgentVisual>)[id] ??
    MANAGED_AGENT_VISUALS[id];
  const override = overrides.get(id);
  const label = override?.label ?? builtin?.label ?? labelFromId(id) ?? agentId;

  return {
    label,
    shortLabel:
      override?.shortLabel ??
      (override?.label ? override.label : (builtin?.shortLabel ?? label)),
    icon: override?.icon ?? builtin?.icon ?? id,
    color: override?.color ?? builtin?.color,
    iconClassName: override?.iconClassName ?? builtin?.iconClassName,
  };
}

/**
 * Register or replace visuals for an Agent discovered at runtime.
 * Returns a cleanup function that restores the previous registration.
 */
export function registerAgentVisual(
  agentId: string,
  visual: Partial<AgentVisual>,
): () => void {
  const id = normalizeAgentId(agentId);
  if (!id) throw new Error("Agent ID cannot be empty");

  const previous = overrides.get(id);
  overrides.set(id, { ...visual });
  notifyVisualChange();

  return () => {
    if (previous) overrides.set(id, previous);
    else overrides.delete(id);
    notifyVisualChange();
  };
}

export function useAgentVisualRegistryRevision(): void {
  useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => revision,
    () => revision,
  );
}

interface AgentIconProps {
  agentId: string;
  size?: number | string;
  className?: string;
  showFallback?: boolean;
}

export function AgentIcon({
  agentId,
  size = 32,
  className,
  showFallback = true,
}: AgentIconProps) {
  useAgentVisualRegistryRevision();
  const visual = getAgentVisual(agentId);

  return (
    <ProviderIcon
      icon={visual.icon}
      name={visual.label}
      color={visual.color}
      size={size}
      className={cn(visual.iconClassName, className)}
      showFallback={showFallback}
    />
  );
}
