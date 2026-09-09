import { invoke } from "@tauri-apps/api/core";

export type ProviderCenterApp =
  | "claude"
  | "claude-desktop"
  | "codex"
  | "gemini"
  | "grokbuild"
  | "opencode"
  | "openclaw"
  | "hermes"
  | "pi";

export interface ProviderDefinition {
  id: string;
  name: string;
  protocol: string;
  baseUrl: string;
  models: string[];
  discoveredModels: string[];
  notes: string;
  enabled: boolean;
  revision: number;
  source?: {
    sourceApp: string;
    sourceRef: string;
    importedAt: number;
  };
  credentialConfigured: boolean;
  credentialHint?: string;
  lastDiscoveryAt?: number;
  lastDiscoveryError?: string;
  createdAt: number;
  updatedAt: number;
}

export interface ProviderBinding {
  providerId: string;
  appType: string;
  status:
    | "pending"
    | "applied"
    | "overridden"
    | "unsupported"
    | "drifted"
    | "detached"
    | "failed";
  enabled: boolean;
  overrideEnabled: boolean;
  appliedRevision?: number;
  expectedFingerprint?: string;
  lastError?: string;
  lastTransactionId?: string;
  updatedAt: number;
}

export interface ProviderCenterState {
  definitions: ProviderDefinition[];
  bindings: ProviderBinding[];
  transactions: ProviderApplyTransaction[];
}

export interface ImportCandidate {
  sourceRef: string;
  sourceApp: string;
  name: string;
  protocol: string;
  baseUrl: string;
  models: string[];
  credentialConfigured: boolean;
  credentialHint?: string;
}

export interface SaveProviderDefinitionInput {
  id?: string;
  name: string;
  protocol: string;
  baseUrl: string;
  models: string[];
  notes?: string;
  enabled?: boolean;
  expectedRevision?: number;
  credentialAction?: "keep" | "replace" | "clear";
  /** Write-only. It is never returned by any Provider Center API. */
  apiKey?: string;
  appTypes: string[];
}

export interface ModelDiscoveryResult {
  providerId: string;
  models: string[];
  discoveredAt: number;
  error?: string;
}

export interface UnifiedModelCatalogEntry {
  id: string;
  appType: string;
  providerId: string;
  providerName: string;
  modelId: string;
  protocol: string;
  sourceType: "apiKey" | "nativeAccount";
  isDefault: boolean;
}

export interface UnifiedModelCatalog {
  entries: UnifiedModelCatalogEntry[];
  generatedAt: number;
}

export interface ProviderApplyPreviewTarget {
  appType: string;
  operation: "create" | "update" | "unsupported";
  compatible: boolean;
  drifted: boolean;
  currentProviderId?: string;
  message?: string;
}

export interface ProviderApplyPreview {
  token: string;
  providerId: string;
  providerRevision: number;
  targets: ProviderApplyPreviewTarget[];
  createdAt: number;
}

export interface ProviderApplyTargetResult {
  appType: string;
  status: string;
  message?: string;
}

export interface ProviderApplyTransaction {
  id: string;
  providerId: string;
  providerRevision: number;
  status: string;
  targets: ProviderApplyTargetResult[];
  createdAt: number;
  completedAt?: number;
}

export const providerCenterApi = {
  get: (): Promise<ProviderCenterState> => invoke("get_provider_center"),
  getModelCatalog: (appTypes: string[] = []): Promise<UnifiedModelCatalog> =>
    invoke("get_provider_center_model_catalog", { appTypes }),
  save: (input: SaveProviderDefinitionInput): Promise<ProviderDefinition> =>
    invoke("save_provider_center_definition", { input }),
  delete: (providerId: string): Promise<void> =>
    invoke("delete_provider_center_definition", { providerId }),
  duplicate: (providerId: string): Promise<ProviderDefinition> =>
    invoke("duplicate_provider_center_definition", { providerId }),
  discoverModels: (providerId: string): Promise<ModelDiscoveryResult> =>
    invoke("discover_provider_center_models", { providerId }),
  scanImports: (): Promise<ImportCandidate[]> =>
    invoke("scan_provider_center_imports"),
  importCandidate: (
    sourceRef: string,
    appTypes: string[],
  ): Promise<ProviderDefinition> =>
    invoke("import_provider_center_candidate", { sourceRef, appTypes }),
  apply: (providerId: string, appTypes: string[]): Promise<ProviderBinding[]> =>
    invoke("apply_provider_center_bindings", { providerId, appTypes }),
  previewApply: (
    providerId: string,
    appTypes: string[],
  ): Promise<ProviderApplyPreview> =>
    invoke("preview_provider_center_apply", { providerId, appTypes }),
  applyTransaction: (
    providerId: string,
    appTypes: string[],
    previewToken: string,
  ): Promise<ProviderApplyTransaction> =>
    invoke("apply_provider_center_transaction", {
      providerId,
      appTypes,
      previewToken,
    }),
  restoreTransaction: (
    transactionId: string,
  ): Promise<ProviderApplyTransaction> =>
    invoke("restore_provider_center_transaction", { transactionId }),
  setOverride: (
    providerId: string,
    appType: string,
    enabled: boolean,
  ): Promise<void> =>
    invoke("set_provider_center_binding_override", {
      providerId,
      appType,
      enabled,
    }),
};
