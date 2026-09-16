import type { AppId } from "@/lib/api";
import type { Provider } from "@/types";
import { isNativeAccountProvider } from "@/utils/providerCapabilities";

export type ProviderListCategory =
  | "universal"
  | "agent-only"
  | "native-account";

export type ProviderListCategories = Readonly<
  Partial<Record<string, ProviderListCategory>>
>;

export interface ProviderCurrentContext {
  currentProviderId: string;
  currentOmoProviderId?: string;
  currentOmoSlimProviderId?: string;
  hermesCurrentProviderId?: string;
  isInConfig: boolean;
  isDefaultModel: boolean;
}

export interface ProviderCurrentState {
  isCurrent: boolean;
  isCurrentUsage: boolean;
}

/**
 * Temporary frontend seam for the forthcoming provider-directory endpoint.
 * Explicit directory data wins. The only universal fallback is the category
 * emitted by existing Provider Center projections; names are never inspected.
 */
export function classifyProviderForList(
  provider: Provider,
  appId: AppId,
  categories?: ProviderListCategories,
): ProviderListCategory {
  const explicit = categories?.[provider.id];
  if (explicit) return explicit;

  if (isNativeAccountProvider(appId, provider)) return "native-account";
  if ((provider.category as string | undefined) === "provider-center") {
    return "universal";
  }
  return "agent-only";
}

/** Preserve each Agent's existing current/config semantics for cards and summary. */
export function resolveProviderCurrentState(
  provider: Provider,
  appId: AppId,
  context: ProviderCurrentContext,
): ProviderCurrentState {
  const isOmo = provider.category === "omo";
  const isOmoSlim = provider.category === "omo-slim";
  const isOmoCurrent =
    isOmo && provider.id === (context.currentOmoProviderId ?? "");
  const isOmoSlimCurrent =
    isOmoSlim && provider.id === (context.currentOmoSlimProviderId ?? "");
  const isHermesCurrent =
    appId === "hermes" &&
    provider.id === (context.hermesCurrentProviderId ?? "");

  const isCurrent =
    appId === "pi"
      ? false
      : isOmo
        ? isOmoCurrent
        : isOmoSlim
          ? isOmoSlimCurrent
          : appId === "hermes"
            ? isHermesCurrent
            : provider.id === context.currentProviderId;

  const isCurrentUsage =
    appId === "pi"
      ? context.isInConfig
      : appId === "opencode"
        ? isOmo || isOmoSlim
          ? isCurrent
          : context.isInConfig
        : appId === "openclaw"
          ? context.isDefaultModel
          : appId === "hermes"
            ? isHermesCurrent
            : isCurrent;

  return { isCurrent, isCurrentUsage };
}
