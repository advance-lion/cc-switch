import { useCallback, useEffect, useMemo, useState } from "react";
import {
  CheckCircle2,
  DatabaseZap,
  Loader2,
  Plus,
  Radar,
  ServerCog,
} from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import type { AppId } from "@/lib/api";
import {
  providerCenterApi,
  type ImportCandidate,
  type ProviderApplyPreview,
  type ProviderBinding,
  type ProviderDefinition,
} from "@/lib/api/providerCenter";
import { refreshProviderCenterApplyCaches } from "@/lib/query/providerCenter";
import { extractErrorMessage } from "@/utils/errorUtils";
import { cn } from "@/lib/utils";

export type AddProviderPath =
  | "create-universal"
  | "use-universal"
  | "agent-only"
  | "scan";

const pathIcons = {
  "create-universal": ServerCog,
  "use-universal": DatabaseZap,
  "agent-only": Plus,
  scan: Radar,
} as const;

const protocolsByApp: Record<AppId, readonly string[]> = {
  claude: ["anthropic"],
  "claude-desktop": ["anthropic"],
  codex: ["openai-responses"],
  gemini: ["gemini"],
  grokbuild: ["openai-chat"],
  opencode: ["openai-chat", "ollama"],
  openclaw: [
    "openai-chat",
    "openai-responses",
    "anthropic",
    "gemini",
    "ollama",
  ],
  hermes: ["openai-chat", "ollama"],
  pi: ["openai-chat", "openai-responses", "anthropic", "gemini", "ollama"],
};

const defaultProtocolByApp: Record<AppId, string> = {
  claude: "anthropic",
  "claude-desktop": "anthropic",
  codex: "openai-responses",
  gemini: "gemini",
  grokbuild: "openai-chat",
  opencode: "openai-chat",
  openclaw: "openai-chat",
  hermes: "openai-chat",
  pi: "openai-chat",
};

const protocolLabels: Record<string, string> = {
  "openai-chat": "OpenAI Chat Completions",
  "openai-responses": "OpenAI Responses",
  anthropic: "Anthropic Messages",
  gemini: "Gemini",
  ollama: "Ollama",
};

interface ProviderCenterAddFlowProps {
  appId: AppId;
  path: Exclude<AddProviderPath, "agent-only">;
  onPathChange: (path: AddProviderPath) => void;
  onComplete: () => void;
}

function PathChooser({
  value,
  onChange,
}: {
  value: AddProviderPath;
  onChange: (path: AddProviderPath) => void;
}) {
  const { t } = useTranslation();
  const paths: AddProviderPath[] = [
    "create-universal",
    "use-universal",
    "agent-only",
    "scan",
  ];

  return (
    <div className="grid gap-3 lg:grid-cols-4">
      {paths.map((path) => {
        const Icon = pathIcons[path];
        const active = value === path;
        return (
          <button
            key={path}
            type="button"
            aria-pressed={active}
            onClick={() => onChange(path)}
            className={cn(
              "rounded-xl border p-4 text-left transition-colors",
              active
                ? "border-primary bg-primary/10 text-foreground"
                : "border-border/70 bg-card hover:bg-muted/50",
            )}
          >
            <Icon className="mb-3 h-5 w-5 text-primary" />
            <div className="text-sm font-medium">
              {t(`provider.addPaths.${path}.title`)}
            </div>
            <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
              {t(`provider.addPaths.${path}.description`)}
            </p>
          </button>
        );
      })}
    </div>
  );
}

export function ProviderAddPathChooser({
  value,
  onChange,
}: {
  value: AddProviderPath;
  onChange: (path: AddProviderPath) => void;
}) {
  return <PathChooser value={value} onChange={onChange} />;
}

export function ProviderCenterAddFlow({
  appId,
  path,
  onPathChange,
  onComplete,
}: ProviderCenterAddFlowProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [definitions, setDefinitions] = useState<ProviderDefinition[]>([]);
  const [bindings, setBindings] = useState<ProviderBinding[]>([]);
  const [selectedId, setSelectedId] = useState("");
  const [preview, setPreview] = useState<ProviderApplyPreview | null>(null);
  const [busy, setBusy] = useState(false);
  const [candidates, setCandidates] = useState<ImportCandidate[]>([]);
  const [importSessionId, setImportSessionId] = useState<string | null>(null);
  const [scanStarted, setScanStarted] = useState(false);
  const [newlyAttachedBinding, setNewlyAttachedBinding] = useState<{
    providerId: string;
    appType: string;
  } | null>(null);
  const [form, setForm] = useState({
    name: "",
    baseUrl: "",
    protocol: defaultProtocolByApp[appId],
    apiKey: "",
    models: "",
    notes: "",
  });

  const loadDefinitions = useCallback(async () => {
    const state = await providerCenterApi.get();
    setDefinitions(
      state.definitions.filter((definition) => definition.enabled),
    );
    setBindings(state.bindings);
  }, []);

  useEffect(() => {
    void loadDefinitions().catch((error) => {
      toast.error(t("providerCenter.loadFailed", { error: String(error) }));
    });
  }, [loadDefinitions, t]);

  useEffect(() => {
    setPreview(null);
    setForm((current) =>
      protocolsByApp[appId].includes(current.protocol)
        ? current
        : { ...current, protocol: defaultProtocolByApp[appId] },
    );
  }, [appId, path, selectedId]);

  // Clean up orphaned pending binding when preview is cleared without applying.
  // This ensures canceling a preview does not leave an enabled/pending binding.
  useEffect(() => {
    if (!preview && newlyAttachedBinding) {
      const { providerId, appType } = newlyAttachedBinding;
      void providerCenterApi
        .disableBinding(providerId, appType, true)
        .then(() => loadDefinitions())
        .catch(() => {})
        .finally(() => setNewlyAttachedBinding(null));
    }
  }, [preview, newlyAttachedBinding, loadDefinitions]);

  const selectedDefinition = useMemo(
    () => definitions.find((definition) => definition.id === selectedId),
    [definitions, selectedId],
  );
  const selectedBinding = useMemo(
    () =>
      bindings.find(
        (binding) =>
          binding.providerId === selectedId && binding.appType === appId,
      ),
    [appId, bindings, selectedId],
  );
  const selectedDefinitionError = useMemo(() => {
    if (!selectedDefinition) return null;
    if (!protocolsByApp[appId].includes(selectedDefinition.protocol)) {
      return t("providerCenter.preview.incompatible");
    }
    if (
      selectedDefinition.protocol !== "ollama" &&
      !selectedDefinition.credentialConfigured
    ) {
      return t("providerCenter.form.requireApiKey");
    }
    return null;
  }, [appId, selectedDefinition, t]);

  const createDefinition = async () => {
    if (!form.name.trim() || !form.baseUrl.trim()) {
      toast.error(t("providerCenter.form.requireNameUrl"));
      return;
    }
    if (form.protocol !== "ollama" && !form.apiKey.trim()) {
      toast.error(t("providerCenter.form.requireApiKey"));
      return;
    }

    setBusy(true);
    try {
      const definition = await providerCenterApi.save({
        name: form.name.trim(),
        baseUrl: form.baseUrl.trim(),
        protocol: form.protocol,
        models: form.models
          .split(/[\n,]/)
          .map((model) => model.trim())
          .filter(Boolean),
        notes: form.notes.trim(),
        enabled: true,
        credentialAction: form.protocol === "ollama" ? "clear" : "replace",
        apiKey: form.protocol === "ollama" ? undefined : form.apiKey.trim(),
        appTypes: [appId],
      });
      setDefinitions((current) => [
        definition,
        ...current.filter((item) => item.id !== definition.id),
      ]);
      setSelectedId(definition.id);
      setBindings((current) => [
        {
          providerId: definition.id,
          appType: appId,
          status: "pending",
          enabled: true,
          overrideEnabled: false,
          updatedAt: Date.now(),
        },
        ...current.filter(
          (binding) =>
            binding.providerId !== definition.id || binding.appType !== appId,
        ),
      ]);
      setPreview(await providerCenterApi.previewApply(definition.id, [appId]));
      toast.success(t("provider.addPaths.savedPendingApply"));
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setBusy(false);
    }
  };

  const previewExisting = async () => {
    if (!selectedId || selectedDefinitionError) return;
    setBusy(true);
    try {
      if (!selectedBinding?.enabled) {
        const attached = await providerCenterApi.attach(selectedId, appId);
        setBindings((current) => [
          ...current.filter(
            (binding) =>
              binding.providerId !== attached.providerId ||
              binding.appType !== attached.appType,
          ),
          attached,
        ]);
        setNewlyAttachedBinding({
          providerId: attached.providerId,
          appType: attached.appType,
        });
      } else {
        setNewlyAttachedBinding(null);
      }
      setPreview(await providerCenterApi.previewApply(selectedId, [appId]));
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setBusy(false);
    }
  };

  const confirmApply = async () => {
    if (!preview) return;
    setBusy(true);
    try {
      const result = await providerCenterApi.applyTransaction(
        preview.providerId,
        [appId],
        preview.token,
      );
      if (result.status !== "applied") {
        throw new Error(t("providerCenter.applyPartial"));
      }
      setNewlyAttachedBinding(null);
      await refreshProviderCenterApplyCaches(queryClient, appId);
      toast.success(t("providerCenter.applySuccess"));
      onComplete();
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setBusy(false);
    }
  };

  const scan = async () => {
    setBusy(true);
    setScanStarted(true);
    try {
      const session = await providerCenterApi.startImportSession([appId]);
      setImportSessionId(session.id);
      setCandidates(
        session.candidates.filter(
          (candidate) => candidate.sourceKind !== "providerCenterProjection",
        ),
      );
      if (session.errors.length > 0) {
        toast.warning(
          session.errors.map((failure) => failure.message).join("; "),
        );
      }
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setBusy(false);
    }
  };

  const importCandidate = async (candidate: ImportCandidate) => {
    if (!importSessionId) return;
    setBusy(true);
    try {
      const result = await providerCenterApi.commitImportCandidate(
        importSessionId,
        candidate.id,
        [appId],
        { action: "createCopy" },
      );
      const definition = result.provider;
      if (!definition) {
        throw new Error(
          t("providerCenter.import.failed", { error: "missing provider" }),
        );
      }
      const attached = await providerCenterApi.attach(definition.id, appId);
      setCandidates((current) =>
        current.filter((item) => item.id !== candidate.id),
      );
      await loadDefinitions();
      setSelectedId(definition.id);
      setBindings((current) => [
        ...current.filter(
          (binding) =>
            binding.providerId !== attached.providerId ||
            binding.appType !== attached.appType,
        ),
        attached,
      ]);
      setNewlyAttachedBinding({
        providerId: attached.providerId,
        appType: attached.appType,
      });
      setPreview(await providerCenterApi.previewApply(definition.id, [appId]));
      toast.success(
        t("providerCenter.import.success", { name: candidate.name }),
      );
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-6">
      <PathChooser value={path} onChange={onPathChange} />

      {path === "create-universal" && (
        <section className="space-y-4 rounded-xl border border-border/70 bg-card p-5">
          <div>
            <h3 className="font-semibold">
              {t("provider.addPaths.create-universal.title")}
            </h3>
            <p className="mt-1 text-sm text-muted-foreground">
              {t("provider.addPaths.createUniversalHint", {
                app: t(`apps.${appId}`),
              })}
            </p>
          </div>
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="shared-provider-name">
                {t("providerCenter.form.name")}
              </Label>
              <Input
                id="shared-provider-name"
                value={form.name}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    name: event.target.value,
                  }))
                }
                placeholder={t("providerCenter.form.namePlaceholder")}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="shared-provider-protocol">
                {t("providerCenter.form.protocol")}
              </Label>
              <select
                id="shared-provider-protocol"
                className="flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                value={form.protocol}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    protocol: event.target.value,
                  }))
                }
              >
                {protocolsByApp[appId].map((protocol) => (
                  <option key={protocol} value={protocol}>
                    {protocolLabels[protocol] ?? protocol}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="space-y-2">
            <Label htmlFor="shared-provider-url">
              {t("providerCenter.form.baseUrl")}
            </Label>
            <Input
              id="shared-provider-url"
              value={form.baseUrl}
              onChange={(event) =>
                setForm((current) => ({
                  ...current,
                  baseUrl: event.target.value,
                }))
              }
              placeholder="https://api.example.com/v1"
            />
          </div>
          {form.protocol !== "ollama" && (
            <div className="space-y-2">
              <Label htmlFor="shared-provider-key">API Key</Label>
              <Input
                id="shared-provider-key"
                type="password"
                autoComplete="off"
                value={form.apiKey}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    apiKey: event.target.value,
                  }))
                }
                placeholder={t("providerCenter.form.credReplacePlaceholder")}
              />
            </div>
          )}
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="shared-provider-models">
                {t("providerCenter.form.models")}
              </Label>
              <Textarea
                id="shared-provider-models"
                value={form.models}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    models: event.target.value,
                  }))
                }
                placeholder={t("providerCenter.form.modelsHint")}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="shared-provider-notes">
                {t("providerCenter.form.notes")}
              </Label>
              <Textarea
                id="shared-provider-notes"
                value={form.notes}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    notes: event.target.value,
                  }))
                }
                placeholder={t("providerCenter.form.notesPlaceholder")}
              />
            </div>
          </div>
          <div className="flex justify-end">
            <Button onClick={() => void createDefinition()} disabled={busy}>
              {busy ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <CheckCircle2 className="mr-2 h-4 w-4" />
              )}
              {t("provider.addPaths.saveAndPreview")}
            </Button>
          </div>
        </section>
      )}

      {path === "use-universal" && (
        <section className="space-y-4 rounded-xl border border-border/70 bg-card p-5">
          <div>
            <h3 className="font-semibold">
              {t("provider.addPaths.use-universal.title")}
            </h3>
            <p className="mt-1 text-sm text-muted-foreground">
              {t("provider.addPaths.useUniversalHint", {
                app: t(`apps.${appId}`),
              })}
            </p>
          </div>
          {definitions.length === 0 ? (
            <p className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">
              {t("provider.addPaths.noUniversal")}
            </p>
          ) : (
            <div className="flex gap-3">
              <select
                aria-label={t("provider.addPaths.selectUniversal")}
                className="flex h-10 min-w-0 flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm"
                value={selectedId}
                onChange={(event) => setSelectedId(event.target.value)}
              >
                <option value="">
                  {t("provider.addPaths.selectUniversal")}
                </option>
                {definitions.map((definition) => (
                  <option key={definition.id} value={definition.id}>
                    {definition.name} · {definition.protocol}
                  </option>
                ))}
              </select>
              <Button
                onClick={() => void previewExisting()}
                disabled={
                  !selectedDefinition ||
                  Boolean(selectedDefinitionError) ||
                  busy
                }
              >
                {busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                {t("providerCenter.previewApply")}
              </Button>
            </div>
          )}
          {selectedDefinitionError && (
            <p role="alert" className="text-sm text-destructive">
              {selectedDefinitionError}
            </p>
          )}
        </section>
      )}

      {path === "scan" && (
        <section className="space-y-4 rounded-xl border border-border/70 bg-card p-5">
          <div>
            <h3 className="font-semibold">
              {t("provider.addPaths.scan.title")}
            </h3>
            <p className="mt-1 text-sm text-muted-foreground">
              {t("provider.addPaths.scanReadOnly", {
                app: t(`apps.${appId}`),
              })}
            </p>
          </div>
          {!scanStarted && (
            <Button onClick={() => void scan()} disabled={busy}>
              {busy ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <Radar className="mr-2 h-4 w-4" />
              )}
              {t("providerCenter.scanLocal")}
            </Button>
          )}
          {scanStarted && candidates.length === 0 && !busy && (
            <p className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">
              {t("providerCenter.import.empty")}
            </p>
          )}
          <div className="space-y-3">
            {candidates.map((candidate) => (
              <div
                key={candidate.id || candidate.sourceRef}
                className="flex items-center justify-between gap-4 rounded-lg border border-border/70 p-4"
              >
                <div className="min-w-0">
                  <div className="font-medium">{candidate.name}</div>
                  <div className="truncate text-xs text-muted-foreground">
                    {candidate.baseUrl} · {candidate.protocol}
                  </div>
                </div>
                <Button
                  size="sm"
                  onClick={() => void importCandidate(candidate)}
                  disabled={busy || candidate.conflicts.length > 0}
                >
                  {t("provider.addPaths.importUniversal")}
                </Button>
              </div>
            ))}
          </div>
        </section>
      )}

      {preview && (
        <section className="space-y-4 rounded-xl border border-primary/30 bg-primary/5 p-5">
          <div>
            <h3 className="font-semibold">
              {t("providerCenter.preview.title", {
                name:
                  definitions.find((item) => item.id === preview.providerId)
                    ?.name ?? "Provider",
              })}
            </h3>
            <p className="mt-1 text-sm text-muted-foreground">
              {t("providerCenter.preview.description")}
            </p>
          </div>
          <div className="space-y-2">
            {preview.targets.map((target) => (
              <div
                key={target.appType}
                className="rounded-lg border border-border/70 bg-background px-3 py-2 text-sm"
              >
                <div className="flex items-center justify-between">
                  <span>{t(`apps.${target.appType}`)}</span>
                  <span
                    className={cn(
                      "text-xs",
                      target.compatible && !target.drifted
                        ? "text-emerald-600"
                        : "text-destructive",
                    )}
                  >
                    {target.drifted
                      ? t("providerCenter.preview.drifted")
                      : target.compatible
                        ? t("providerCenter.preview.compatible")
                        : t("providerCenter.preview.incompatible")}
                  </span>
                </div>
                {target.message && (
                  <p className="mt-1 text-xs text-muted-foreground">
                    {target.message}
                  </p>
                )}
              </div>
            ))}
          </div>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setPreview(null)}>
              {t("common.cancel")}
            </Button>
            <Button
              onClick={() => void confirmApply()}
              disabled={
                busy ||
                preview.targets.some(
                  (target) => !target.compatible || target.drifted,
                )
              }
            >
              {busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {t("providerCenter.preview.confirm")}
            </Button>
          </div>
        </section>
      )}
    </div>
  );
}
