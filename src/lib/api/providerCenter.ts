import { invoke } from "@tauri-apps/api/core";
import type { Provider } from "@/types";

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
    sourceFingerprint?: string;
    lastObservedAt?: number;
  };
  sources: Array<{
    sourceApp: string;
    sourceRef: string;
    importedAt: number;
    sourceFingerprint?: string;
    lastObservedAt?: number;
  }>;
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

export type AgentProviderScope = "universal" | "agentOnly" | "nativeAccount";

export type AgentProviderOwnership =
  | "providerCenterProjection"
  | "ccSwitchManaged"
  | "agentNative";

export interface AgentProviderCatalogItem {
  providerId: string;
  providerName: string;
  category?: string;
  scope: AgentProviderScope;
  ownership: AgentProviderOwnership;
  definitionId?: string;
  bindingStatus?: ProviderBinding["status"];
  appliedRevision?: number;
  drifted: boolean;
  readOnly: boolean;
}

export interface AgentProviderCatalog {
  appType: ProviderCenterApp;
  items: AgentProviderCatalogItem[];
  generatedAt: number;
}

export interface ImportConflict {
  existingProviderId: string;
  existingName: string;
  existingRevision: number;
  reasons: string[];
}

export type ImportCommitAction = "createCopy" | "merge" | "skip";

export interface ImportCommitDecision {
  action: ImportCommitAction;
  targetProviderId?: string;
  expectedRevision?: number;
}

export interface ImportCommitResult {
  action: ImportCommitAction;
  provider?: ProviderDefinition;
  repeated: boolean;
}

export interface ImportCandidate {
  id: string;
  sessionId: string;
  sourceRef: string;
  sourceApp: string;
  sourceKind: "externalManaged" | "providerCenterProjection" | "localManaged";
  name: string;
  protocol: string;
  baseUrl: string;
  models: string[];
  credentialConfigured: boolean;
  credentialHint?: string;
  conflicts: ImportConflict[];
}

export interface ImportFailure {
  appType: string;
  sourceRef?: string;
  code: string;
  stage: string;
  message: string;
}

export interface ProviderImportSession {
  id: string;
  state: "ready" | "readyWithErrors" | "completed" | "expired" | string;
  candidates: ImportCandidate[];
  errors: ImportFailure[];
  createdAt: number;
  expiresAt: number;
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
  connectionMode: "direct" | "proxy" | "unsupported";
  routeId?: string;
  requiresTakeover: boolean;
  drifted: boolean;
  currentProviderId?: string;
  liveFingerprint?: string;
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

export interface ManagedProviderDraftInput {
  appType: string;
  provider: Provider;
  definitionId: string;
  expectedRevision?: number;
  /** Agents to save the universal provider to. If empty, defaults to [appType]. */
  targetAppTypes?: string[];
}

export type DeleteMode = "removeCurrent" | "detachKeepIndependent" | "deleteGlobally";

export const providerCenterApi = {
  get: (): Promise<ProviderCenterState> => invoke("get_provider_center"),
  getAgentProviderCatalog: (
    appType: ProviderCenterApp,
  ): Promise<AgentProviderCatalog> =>
    invoke("get_provider_center_agent_provider_catalog", { appType }),
  getModelCatalog: (appTypes: string[] = []): Promise<UnifiedModelCatalog> =>
    invoke("get_provider_center_model_catalog", { appTypes }),
  save: (input: SaveProviderDefinitionInput): Promise<ProviderDefinition> =>
    invoke("save_provider_center_definition", { input }),
  delete: (providerId: string): Promise<void> =>
    invoke("delete_provider_center_definition", { providerId }),
  deleteProvider: (
    providerId: string,
    appType: string,
    mode: DeleteMode,
  ): Promise<void> =>
    invoke("delete_provider_center_provider", { providerId, appType, mode }),
  duplicate: (providerId: string): Promise<ProviderDefinition> =>
    invoke("duplicate_provider_center_definition", { providerId }),
  discoverModels: (providerId: string): Promise<ModelDiscoveryResult> =>
    invoke("discover_provider_center_models", { providerId }),
  scanImports: (): Promise<ImportCandidate[]> =>
    invoke("scan_provider_center_imports"),
  startImportSession: (
    appTypes: string[] = [],
  ): Promise<ProviderImportSession> =>
    invoke("start_provider_center_import_session", { appTypes }),
  getImportSession: (sessionId: string): Promise<ProviderImportSession> =>
    invoke("get_provider_center_import_session", { sessionId }),
  commitImportCandidate: (
    sessionId: string,
    candidateId: string,
    appTypes: string[],
    decision: ImportCommitDecision,
  ): Promise<ImportCommitResult> =>
    invoke("commit_provider_center_import_candidate", {
      sessionId,
      candidateId,
      appTypes,
      decision,
    }),
  importCandidate: (
    sourceRef: string,
    appTypes: string[],
  ): Promise<ProviderDefinition> =>
    invoke("import_provider_center_candidate", { sourceRef, appTypes }),
  attach: (
    providerId: string,
    appType: ProviderCenterApp,
  ): Promise<ProviderBinding> =>
    invoke("attach_provider_center_binding", { providerId, appType }),
  apply: (providerId: string, appTypes: string[]): Promise<ProviderBinding[]> =>
    invoke("apply_provider_center_bindings", { providerId, appTypes }),
  previewApply: (
    providerId: string,
    appTypes: string[],
  ): Promise<ProviderApplyPreview> =>
    invoke("preview_provider_center_apply", { providerId, appTypes }),
  previewDefinitionCompatibility: (
    providerId: string,
    appTypes: string[],
  ): Promise<ProviderApplyPreview> =>
    invoke("preview_provider_center_definition_compatibility", {
      providerId,
      appTypes,
    }),
  applyTransaction: (
    providerId: string,
    appTypes: string[],
    previewToken: string,
    idempotencyKey = crypto.randomUUID(),
  ): Promise<ProviderApplyTransaction> =>
    invoke("apply_provider_center_transaction", {
      providerId,
      appTypes,
      previewToken,
      idempotencyKey,
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
  disableBinding: (
    providerId: string,
    appType: string,
    removeProjection = false,
  ): Promise<void> =>
    invoke("disable_provider_center_binding", {
      providerId,
      appType,
      removeProjection,
    }),
  previewManagedDraft: (
    input: ManagedProviderDraftInput,
  ): Promise<ProviderApplyPreview> =>
    invoke("preview_provider_center_managed_draft", { input }),
  confirmManagedDraft: (
    input: ManagedProviderDraftInput,
    previewToken: string,
    idempotencyKey = crypto.randomUUID(),
  ): Promise<ProviderApplyTransaction> =>
    invoke("confirm_provider_center_managed_draft", {
      input,
      previewToken,
      idempotencyKey,
    }),
};
