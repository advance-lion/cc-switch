import { settingsApi } from "@/lib/api";

export interface ManagedAgent {
  /** Stable UI identity. It is intentionally separate from provider AppId. */
  id: string;
  tool: string;
  name: string;
  shortLabel: string;
  icon: string;
  packageName: string;
  version: string | null;
  installedButBroken: boolean;
  error: string | null;
  providerIntegration: "unsupported" | "planned" | "supported";
}

type ManagedAgentAdapter = Omit<
  ManagedAgent,
  "version" | "installedButBroken" | "error"
>;

/**
 * Runtime Agent adapters live outside AppId because AppId means “has a tested
 * Provider/config adapter”.  Adding a lifecycle-only Agent must not make the
 * Provider UI write another application's config by accident.
 */
export const MANAGED_AGENT_ADAPTERS: readonly ManagedAgentAdapter[] = [
  {
    id: "qoder",
    tool: "qoder",
    name: "Qoder",
    shortLabel: "Qoder",
    icon: "qoder",
    packageName: "@qoder-ai/qodercli",
    providerIntegration: "planned",
  },
] as const;

/** Detect installed, runnable Agents that have a lifecycle adapter. */
export async function discoverManagedAgents(): Promise<ManagedAgent[]> {
  const versions = await settingsApi.getToolVersions(
    MANAGED_AGENT_ADAPTERS.map((agent) => agent.tool),
  );
  const byTool = new Map(versions.map((status) => [status.name, status]));

  return MANAGED_AGENT_ADAPTERS.flatMap((agent) => {
    const status = byTool.get(agent.tool);
    if (!status?.version && !status?.installed_but_broken) return [];
    return [
      {
        ...agent,
        version: status.version,
        installedButBroken: status.installed_but_broken,
        error: status.error,
      },
    ];
  });
}
