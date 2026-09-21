import type { QueryClient } from "@tanstack/react-query";
import type { AppId } from "@/lib/api";
import { providersApi } from "@/lib/api/providers";
import { openclawKeys } from "@/hooks/useOpenClaw";
import { hermesKeys } from "@/hooks/useHermes";
import { piKeys } from "@/lib/query/pi";

export const providerCenterKeys = {
  all: ["provider-center"] as const,
  state: ["provider-center", "state"] as const,
  agentCatalogs: ["provider-center", "agent-catalog"] as const,
  agentCatalog: (appId: AppId) =>
    ["provider-center", "agent-catalog", appId] as const,
};

export const providerLiveMembershipKeys = {
  all: ["provider-live-membership"] as const,
  app: (appId: AppId) => ["provider-live-membership", appId] as const,
};

export async function refreshProviderCenterApplyCaches(
  queryClient: QueryClient,
  appId: AppId,
): Promise<void> {
  await refreshProviderCenterApps(queryClient, [appId]);
}

export async function refreshProviderCenterApps(
  queryClient: QueryClient,
  appIds: AppId[],
): Promise<void> {
  const uniqueAppIds = Array.from(new Set(appIds));
  const invalidations: Array<Promise<unknown>> = [
    queryClient.invalidateQueries({ queryKey: providerCenterKeys.all }),
  ];

  for (const appId of uniqueAppIds) {
    invalidations.push(
      queryClient.invalidateQueries({ queryKey: ["providers", appId] }),
      queryClient.invalidateQueries({
        queryKey: providerCenterKeys.agentCatalog(appId),
      }),
    );

    if (appId === "opencode") {
      invalidations.push(
        queryClient.invalidateQueries({
          queryKey: ["opencodeLiveProviderIds"],
        }),
        queryClient.invalidateQueries({
          queryKey: ["opencode", "runtime-models"],
        }),
      );
    } else if (appId === "openclaw") {
      invalidations.push(
        queryClient.invalidateQueries({
          queryKey: openclawKeys.liveProviderIds,
        }),
        queryClient.invalidateQueries({ queryKey: openclawKeys.defaultModel }),
        queryClient.invalidateQueries({ queryKey: openclawKeys.health }),
      );
    } else if (appId === "hermes") {
      invalidations.push(
        queryClient.invalidateQueries({ queryKey: hermesKeys.liveProviderIds }),
        queryClient.invalidateQueries({ queryKey: hermesKeys.modelConfig }),
      );
    } else if (appId === "pi") {
      invalidations.push(
        queryClient.invalidateQueries({ queryKey: piKeys.currentState }),
      );
    } else if (appId === "dsh") {
      invalidations.push(
        queryClient.invalidateQueries({
          queryKey: providerLiveMembershipKeys.app(appId),
        }),
      );
    }
  }

  await Promise.all(invalidations);
  await providersApi.updateTrayMenu().catch((error) => {
    console.error(
      "Failed to update tray menu after Provider Center change",
      error,
    );
  });
}
