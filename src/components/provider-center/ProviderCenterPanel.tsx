import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  CloudCog,
  Copy,
  Download,
  History,
  KeyRound,
  Link2,
  Loader2,
  Pencil,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  ShieldCheck,
  Sparkles,
  Trash2,
  Unlink,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  providerCenterApi,
  type ImportCandidate,
  type ImportFailure,
  type ProviderApplyPreview,
  type ProviderApplyTransaction,
  type ProviderBinding,
  type ProviderDefinition,
  type SaveProviderDefinitionInput,
  type UnifiedModelCatalogEntry,
} from "@/lib/api/providerCenter";
import { settingsApi } from "@/lib/api/settings";

const APPS = [
  { id: "claude", label: "Claude Code", tool: "claude" },
  { id: "claude-desktop", label: "Claude Desktop", desktop: true },
  { id: "codex", label: "Codex", tool: "codex" },
  { id: "gemini", label: "Gemini CLI", tool: "gemini" },
  { id: "grokbuild", label: "Grok Build", tool: "grok" },
  { id: "opencode", label: "OpenCode", tool: "opencode" },
  { id: "openclaw", label: "OpenClaw", tool: "openclaw" },
  { id: "hermes", label: "Hermes", tool: "hermes" },
  { id: "pi", label: "Pi", tool: "pi" },
] as const;

const PROTOCOLS = [
  { id: "openai-responses", label: "OpenAI Responses" },
  { id: "openai-chat", label: "OpenAI Chat Completions" },
  { id: "anthropic", label: "Anthropic Messages" },
  { id: "gemini", label: "Gemini" },
  { id: "ollama", label: "Ollama" },
] as const;

const statusClass: Record<ProviderBinding["status"], string> = {
  pending:
    "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  applied:
    "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  overridden:
    "border-blue-500/30 bg-blue-500/10 text-blue-700 dark:text-blue-300",
  unsupported: "border-muted-foreground/20 bg-muted text-muted-foreground",
  drifted:
    "border-orange-500/30 bg-orange-500/10 text-orange-700 dark:text-orange-300",
  detached: "border-muted-foreground/20 bg-muted/50 text-muted-foreground",
  failed: "border-destructive/30 bg-destructive/10 text-destructive",
};

type CredentialAction = "keep" | "replace" | "clear";

type ProviderFormState = {
  id?: string;
  expectedRevision?: number;
  name: string;
  baseUrl: string;
  protocol: string;
  apiKey: string;
  credentialAction: CredentialAction;
  models: string;
  notes: string;
  enabled: boolean;
  appTypes: string[];
};

const emptyForm = (appTypes: string[] = []): ProviderFormState => ({
  name: "",
  baseUrl: "",
  protocol: "openai-chat",
  apiKey: "",
  credentialAction: "replace",
  models: "",
  notes: "",
  enabled: true,
  appTypes,
});

function uniqueModels(value: string): string[] {
  return Array.from(
    new Set(
      value
        .split(/[\n,]/)
        .map((item) => item.trim())
        .filter(Boolean),
    ),
  );
}

function appLabel(appType: string): string {
  return APPS.find((app) => app.id === appType)?.label ?? appType;
}

function formatTime(value?: number): string {
  if (!value) return "—";
  // Provider Center timestamps are milliseconds since Unix epoch.
  return new Date(value).toLocaleString();
}

function AppChooser({
  apps,
  value,
  onChange,
}: {
  apps: readonly { id: string; label: string }[];
  value: string[];
  onChange: (apps: string[]) => void;
}) {
  return (
    <div className="grid gap-2 sm:grid-cols-2">
      {apps.map((app) => {
        const checked = value.includes(app.id);
        return (
          <label
            key={app.id}
            className="flex cursor-pointer items-center gap-2 rounded-lg border border-border/70 px-3 py-2 text-sm transition-colors hover:bg-muted/50"
          >
            <Checkbox
              checked={checked}
              onCheckedChange={(next) =>
                onChange(
                  next === true
                    ? [...value, app.id]
                    : value.filter((id) => id !== app.id),
                )
              }
            />
            {app.label}
          </label>
        );
      })}
    </div>
  );
}

export function ProviderCenterPanel() {
  const { t } = useTranslation();
  const [definitions, setDefinitions] = useState<ProviderDefinition[]>([]);
  const [bindings, setBindings] = useState<ProviderBinding[]>([]);
  const [transactions, setTransactions] = useState<ProviderApplyTransaction[]>(
    [],
  );
  const [catalog, setCatalog] = useState<UnifiedModelCatalogEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [formOpen, setFormOpen] = useState(false);
  const [form, setForm] = useState<ProviderFormState>(() =>
    emptyForm(["codex"]),
  );
  const [importsOpen, setImportsOpen] = useState(false);
  const [importSessionId, setImportSessionId] = useState<string | null>(null);
  const [importErrors, setImportErrors] = useState<ImportFailure[]>([]);
  const [candidates, setCandidates] = useState<ImportCandidate[]>([]);
  const [importApps, setImportApps] = useState<Record<string, string[]>>({});
  const [preview, setPreview] = useState<ProviderApplyPreview | null>(null);
  const [previewDefinition, setPreviewDefinition] =
    useState<ProviderDefinition | null>(null);
  const [transactionResult, setTransactionResult] =
    useState<ProviderApplyTransaction | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<ProviderDefinition | null>(
    null,
  );
  const [availableApps, setAvailableApps] =
    useState<readonly { id: string; label: string }[]>(APPS);

  const load = useCallback(async () => {
    try {
      const [state, modelCatalog] = await Promise.all([
        providerCenterApi.get(),
        providerCenterApi.getModelCatalog(),
      ]);
      setDefinitions(state.definitions);
      setBindings(state.bindings);
      setTransactions(state.transactions);
      setCatalog(modelCatalog.entries);
    } catch (error) {
      toast.error(t("providerCenter.loadFailed", { error: String(error) }));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);
  useEffect(() => {
    const toolApps = APPS.filter((app) => "tool" in app);
    void Promise.all([
      settingsApi.getToolVersions(toolApps.map((app) => app.tool)),
      settingsApi.getDesktopAppStatus("codex-desktop").catch(() => null),
      settingsApi.getDesktopAppStatus("claude-desktop").catch(() => null),
    ])
      .then(([tools, codexDesktop, claudeDesktop]) => {
        const installedTools = new Set(
          tools
            .filter((tool) => Boolean(tool.version))
            .map((tool) => (tool.name === "grok" ? "grokbuild" : tool.name)),
        );
        const detected = APPS.filter((app) => {
          if (app.id === "claude-desktop")
            return Boolean(claudeDesktop?.installed);
          if (app.id === "codex" && codexDesktop?.installed) return true;
          return installedTools.has(app.id);
        });
        if (detected.length > 0) {
          setAvailableApps(detected);
          setForm((current) => ({
            ...current,
            appTypes: current.appTypes.filter((app) =>
              detected.some((item) => item.id === app),
            ),
          }));
        }
      })
      .catch(() => {
        // 浏览器预览没有完整桌面探测能力；保留完整列表用于界面预览。
      });
  }, []);

  const bindingsFor = useCallback(
    (id: string) => bindings.filter((binding) => binding.providerId === id),
    [bindings],
  );
  const importSummary = useMemo(
    () =>
      candidates.filter((candidate) => candidate.credentialConfigured).length,
    [candidates],
  );

  const openCreate = () => {
    setForm(emptyForm(availableApps[0] ? [availableApps[0].id] : []));
    setFormOpen(true);
  };

  const openEdit = (definition: ProviderDefinition) => {
    setForm({
      id: definition.id,
      expectedRevision: definition.revision,
      name: definition.name,
      baseUrl: definition.baseUrl,
      protocol: definition.protocol,
      apiKey: "",
      credentialAction: "keep",
      models: definition.models.join("\n"),
      notes: definition.notes,
      enabled: definition.enabled,
      appTypes: bindingsFor(definition.id)
        .filter((binding) => binding.enabled)
        .map((binding) => binding.appType),
    });
    setFormOpen(true);
  };

  const save = async () => {
    if (!form.name.trim() || !form.baseUrl.trim()) {
      toast.error(t("providerCenter.form.requireNameUrl"));
      return;
    }
    if (
      form.protocol !== "ollama" &&
      form.credentialAction === "replace" &&
      !form.apiKey.trim()
    ) {
      toast.error(t("providerCenter.form.requireApiKey"));
      return;
    }
    const input: SaveProviderDefinitionInput = {
      id: form.id,
      expectedRevision: form.expectedRevision,
      name: form.name,
      baseUrl: form.baseUrl,
      protocol: form.protocol,
      models: uniqueModels(form.models),
      notes: form.notes,
      enabled: form.enabled,
      credentialAction:
        form.protocol === "ollama" ? "clear" : form.credentialAction,
      apiKey:
        form.protocol !== "ollama" && form.credentialAction === "replace"
          ? form.apiKey
          : undefined,
      appTypes: form.appTypes,
    };
    setBusy(true);
    try {
      await providerCenterApi.save(input);
      setFormOpen(false);
      await load();
      toast.success(
        form.id
          ? t("providerCenter.form.saveSuccessUpdated")
          : t("providerCenter.form.saveSuccessCreated"),
      );
    } catch (error) {
      toast.error(
        t("providerCenter.form.saveFailed", { error: String(error) }),
      );
    } finally {
      setBusy(false);
    }
  };

  const openImports = async () => {
    setBusy(true);
    try {
      const session = await providerCenterApi.startImportSession();
      setImportSessionId(session.id);
      setImportErrors(session.errors);
      setCandidates(session.candidates);
      setImportApps(
        Object.fromEntries(
          session.candidates.map((candidate) => [
            candidate.sourceRef,
            availableApps[0] ? [availableApps[0].id] : [],
          ]),
        ),
      );
      setImportsOpen(true);
    } catch (error) {
      toast.error(t("providerCenter.scanFailed", { error: String(error) }));
    } finally {
      setBusy(false);
    }
  };

  const importCandidate = async (candidate: ImportCandidate) => {
    setBusy(true);
    try {
      if (importSessionId && candidate.id) {
        if (candidate.conflicts.length > 0) {
          toast.error(t("providerCenter.import.conflictUnsupported"));
          return;
        }
        await providerCenterApi.commitImportCandidate(
          importSessionId,
          candidate.id,
          importApps[candidate.sourceRef] ?? [],
          { action: "createCopy" },
        );
        const session =
          await providerCenterApi.getImportSession(importSessionId);
        setCandidates(session.candidates);
      } else {
        await providerCenterApi.importCandidate(
          candidate.sourceRef,
          importApps[candidate.sourceRef] ?? [],
        );
      }
      await load();
      toast.success(
        t("providerCenter.import.success", { name: candidate.name }),
      );
    } catch (error) {
      toast.error(t("providerCenter.import.failed", { error: String(error) }));
    } finally {
      setBusy(false);
    }
  };

  const discoverModels = async (definition: ProviderDefinition) => {
    setBusy(true);
    try {
      const result = await providerCenterApi.discoverModels(definition.id);
      await load();
      if (result.error)
        toast.error(t("providerCenter.discoverFailed"), {
          description: result.error,
        });
      else
        toast.success(
          t("providerCenter.discoverSuccess", { count: result.models.length }),
        );
    } catch (error) {
      toast.error(
        t("providerCenter.discoverFailedDetail", { error: String(error) }),
      );
    } finally {
      setBusy(false);
    }
  };

  const duplicate = async (definition: ProviderDefinition) => {
    setBusy(true);
    try {
      await providerCenterApi.duplicate(definition.id);
      await load();
      toast.success(t("providerCenter.duplicateSuccess"));
    } catch (error) {
      toast.error(
        t("providerCenter.duplicateFailed", { error: String(error) }),
      );
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!deleteTarget) return;
    setBusy(true);
    try {
      await providerCenterApi.delete(deleteTarget.id);
      setDeleteTarget(null);
      await load();
      toast.success(t("providerCenter.deleteSuccess"));
    } catch (error) {
      toast.error(t("providerCenter.deleteFailed", { error: String(error) }));
    } finally {
      setBusy(false);
    }
  };

  const toggleEnabled = async (
    definition: ProviderDefinition,
    enabled: boolean,
  ) => {
    setBusy(true);
    try {
      await providerCenterApi.save({
        id: definition.id,
        expectedRevision: definition.revision,
        name: definition.name,
        baseUrl: definition.baseUrl,
        protocol: definition.protocol,
        models: definition.models,
        notes: definition.notes,
        enabled,
        credentialAction: "keep",
        appTypes: bindingsFor(definition.id)
          .filter((binding) => binding.enabled)
          .map((binding) => binding.appType),
      });
      await load();
      toast.success(
        enabled
          ? t("providerCenter.enableSuccess")
          : t("providerCenter.disableSuccess"),
      );
    } catch (error) {
      toast.error(t("providerCenter.toggleFailed", { error: String(error) }));
    } finally {
      setBusy(false);
    }
  };

  const updateOverride = async (binding: ProviderBinding, enabled: boolean) => {
    try {
      await providerCenterApi.setOverride(
        binding.providerId,
        binding.appType,
        enabled,
      );
      await load();
      toast.success(
        enabled
          ? t("providerCenter.overrideEnabledSuccess")
          : t("providerCenter.overrideDisabledSuccess"),
      );
    } catch (error) {
      toast.error(t("providerCenter.overrideFailed", { error: String(error) }));
    }
  };

  const detachBinding = async (binding: ProviderBinding) => {
    try {
      await providerCenterApi.disableBinding(
        binding.providerId,
        binding.appType,
        false,
      );
      await load();
      toast.success(t("providerCenter.detachSuccess"));
    } catch (error) {
      toast.error(t("providerCenter.detachFailed", { error: String(error) }));
    }
  };

  const prepareApply = async (definition: ProviderDefinition) => {
    const targets = bindingsFor(definition.id)
      .filter((binding) => binding.enabled && !binding.overrideEnabled)
      .map((binding) => binding.appType);
    if (targets.length === 0) {
      toast.error(t("providerCenter.noApplyTargets"));
      return;
    }
    setBusy(true);
    try {
      const next = await providerCenterApi.previewApply(definition.id, targets);
      setPreviewDefinition(definition);
      setPreview(next);
    } catch (error) {
      toast.error(t("providerCenter.previewFailed", { error: String(error) }));
    } finally {
      setBusy(false);
    }
  };

  const confirmApply = async () => {
    if (!preview || !previewDefinition) return;
    setBusy(true);
    try {
      const result = await providerCenterApi.applyTransaction(
        previewDefinition.id,
        preview.targets.map((target) => target.appType),
        preview.token,
      );
      setPreview(null);
      setPreviewDefinition(null);
      setTransactionResult(result);
      await load();
      if (result.status === "applied")
        toast.success(t("providerCenter.applySuccess"));
      else toast.error(t("providerCenter.applyPartial"));
    } catch (error) {
      toast.error(t("providerCenter.applyFailed", { error: String(error) }));
      await load();
    } finally {
      setBusy(false);
    }
  };

  const restore = async (transaction: ProviderApplyTransaction) => {
    setBusy(true);
    try {
      const result = await providerCenterApi.restoreTransaction(transaction.id);
      setTransactionResult(result);
      await load();
      if (result.status === "restored")
        toast.success(t("providerCenter.restoreSuccess"));
      else toast.error(t("providerCenter.restorePartial"));
    } catch (error) {
      toast.error(t("providerCenter.restoreFailed", { error: String(error) }));
    } finally {
      setBusy(false);
    }
  };

  if (loading) {
    return (
      <div className="flex min-h-72 items-center justify-center text-muted-foreground">
        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
        {t("providerCenter.loading")}
      </div>
    );
  }

  const previewBlocked =
    preview?.targets.some((target) => !target.compatible || target.drifted) ??
    false;
  const enabledCount = definitions.filter(
    (definition) => definition.enabled,
  ).length;
  const pendingCount = bindings.filter(
    (binding) =>
      binding.enabled &&
      ["pending", "drifted", "failed"].includes(binding.status),
  ).length;
  const recoveryRequiredTransactions = transactions.filter(
    (transaction) => transaction.status === "recoveryRequired",
  );

  return (
    <div className="space-y-5">
      {recoveryRequiredTransactions.length > 0 && (
        <section className="flex items-start justify-between gap-4 rounded-2xl border border-destructive/40 bg-destructive/5 p-4 text-sm text-destructive">
          <div className="flex gap-2">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <div>
              <p className="font-medium">
                {t("providerCenter.recoveryBannerTitle", {
                  count: recoveryRequiredTransactions.length,
                })}
              </p>
              <p className="mt-1 text-xs opacity-80">
                {t("providerCenter.recoveryBannerHint")}
              </p>
            </div>
          </div>
          <Button
            size="sm"
            variant="outline"
            onClick={() => setHistoryOpen(true)}
          >
            {t("providerCenter.openHistory")}
          </Button>
        </section>
      )}
      <section className="rounded-2xl border border-border/70 bg-card p-5 shadow-sm">
        <div className="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <div className="space-y-2">
            <div className="flex items-center gap-2">
              <div className="rounded-xl bg-primary/10 p-2 text-primary">
                <CloudCog className="h-5 w-5" />
              </div>
              <div>
                <h2 className="text-xl font-semibold">
                  {t("providerCenter.title")}
                </h2>
                <p className="text-sm text-muted-foreground">
                  {t("providerCenter.subtitle")}
                </p>
              </div>
            </div>
            <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
              <Badge variant="secondary">
                {t("providerCenter.badgeServices", {
                  count: definitions.length,
                })}
              </Badge>
              <Badge variant="secondary">
                {t("providerCenter.badgeEnabled", { count: enabledCount })}
              </Badge>
              {pendingCount > 0 && (
                <Badge className="border-amber-500/30 bg-amber-500/10 text-amber-700 hover:bg-amber-500/10 dark:text-amber-300">
                  {t("providerCenter.badgePending", { count: pendingCount })}
                </Badge>
              )}
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="outline"
              onClick={() => void openImports()}
              disabled={busy}
            >
              <Download className="mr-2 h-4 w-4" />
              {t("providerCenter.importLocal")}
            </Button>
            <Button
              variant="outline"
              onClick={() => setHistoryOpen(true)}
              disabled={transactions.length === 0}
            >
              <History className="mr-2 h-4 w-4" />
              {t("providerCenter.history")}
            </Button>
            <Button onClick={openCreate}>
              <Plus className="mr-2 h-4 w-4" />
              {t("providerCenter.addService")}
            </Button>
          </div>
        </div>
        <div className="mt-4 flex gap-2 rounded-xl border border-blue-500/20 bg-blue-500/5 px-3 py-2 text-sm text-muted-foreground">
          <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-blue-600" />
          <p>{t("providerCenter.securityNote")}</p>
        </div>
      </section>

      {catalog.length > 0 && (
        <section className="rounded-2xl border border-border/70 bg-card p-5 shadow-sm">
          <div className="flex items-center gap-2">
            <Sparkles className="h-4 w-4 text-primary" />
            <div>
              <h3 className="font-medium">
                {t("providerCenter.catalogTitle")}
              </h3>
              <p className="text-xs text-muted-foreground">
                {t("providerCenter.catalogSubtitle")}
              </p>
            </div>
          </div>
          <div className="mt-4 grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
            {catalog.map((entry) => (
              <div
                key={entry.id}
                className="rounded-xl border border-border/60 px-3 py-2.5"
              >
                <div className="flex items-center justify-between gap-2">
                  <p className="truncate text-sm font-medium">
                    {entry.modelId}
                  </p>
                  <Badge variant="outline">{appLabel(entry.appType)}</Badge>
                </div>
                <p className="mt-1 truncate text-xs text-muted-foreground">
                  {entry.providerName} ·{" "}
                  {entry.sourceType === "nativeAccount"
                    ? t("providerCenter.nativeAccount")
                    : "API Key"}
                  {entry.isDefault
                    ? ` · ${t("providerCenter.defaultBadge")}`
                    : ""}
                </p>
              </div>
            ))}
          </div>
        </section>
      )}

      {definitions.length === 0 ? (
        <section className="flex min-h-64 flex-col items-center justify-center rounded-2xl border border-dashed border-border bg-muted/20 px-6 text-center">
          <CloudCog className="mb-3 h-10 w-10 text-muted-foreground/60" />
          <h3 className="font-medium">{t("providerCenter.emptyTitle")}</h3>
          <p className="mt-1 max-w-lg text-sm text-muted-foreground">
            {t("providerCenter.emptyHint")}
          </p>
          <div className="mt-4 flex gap-2">
            <Button variant="outline" onClick={() => void openImports()}>
              <Search className="mr-2 h-4 w-4" />
              {t("providerCenter.scanLocal")}
            </Button>
            <Button onClick={openCreate}>
              <Plus className="mr-2 h-4 w-4" />
              {t("providerCenter.addService")}
            </Button>
          </div>
        </section>
      ) : (
        <div className="grid gap-4 xl:grid-cols-2">
          {definitions.map((definition) => {
            const definitionBindings = bindingsFor(definition.id);
            const allModels = Array.from(
              new Set([...definition.models, ...definition.discoveredModels]),
            );
            return (
              <article
                key={definition.id}
                className="rounded-2xl border border-border/70 bg-card p-5 shadow-sm"
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <h3 className="truncate text-lg font-semibold">
                        {definition.name}
                      </h3>
                      <Badge variant="outline">
                        {PROTOCOLS.find(
                          (protocol) => protocol.id === definition.protocol,
                        )?.label ?? definition.protocol}
                      </Badge>
                      {!definition.enabled && (
                        <Badge variant="secondary">
                          {t("providerCenter.disabledBadge")}
                        </Badge>
                      )}
                    </div>
                    <p className="mt-1 break-all text-sm text-muted-foreground">
                      {definition.baseUrl}
                    </p>
                  </div>
                  <Switch
                    checked={definition.enabled}
                    onCheckedChange={(checked) =>
                      void toggleEnabled(definition, checked)
                    }
                    aria-label={t("providerCenter.enableAriaLabel", {
                      name: definition.name,
                    })}
                  />
                </div>

                <div className="mt-4 grid gap-3 sm:grid-cols-2">
                  <div className="rounded-xl bg-muted/40 p-3">
                    <div className="flex items-center gap-2 text-xs text-muted-foreground">
                      <KeyRound className="h-3.5 w-3.5" />
                      {t("providerCenter.credentialLabel")}
                    </div>
                    <p className="mt-1 text-sm font-medium">
                      {definition.credentialConfigured
                        ? `${t("providerCenter.credentialSaved")}${definition.credentialHint ? ` · ${definition.credentialHint}` : ""}`
                        : t("providerCenter.credentialNotConfigured")}
                    </p>
                  </div>
                  <div className="rounded-xl bg-muted/40 p-3">
                    <div className="text-xs text-muted-foreground">
                      {t("providerCenter.modelsLabel")}
                    </div>
                    <p className="mt-1 text-sm font-medium">
                      {allModels.length > 0
                        ? t("providerCenter.modelsCount", {
                            count: allModels.length,
                          })
                        : t("providerCenter.modelsEmpty")}
                    </p>
                  </div>
                </div>

                {allModels.length > 0 && (
                  <div className="mt-3 flex max-h-20 flex-wrap gap-1.5 overflow-y-auto">
                    {allModels.map((model) => (
                      <Badge
                        key={model}
                        variant="secondary"
                        className="font-normal"
                      >
                        {model}
                      </Badge>
                    ))}
                  </div>
                )}
                {definition.notes && (
                  <p className="mt-3 rounded-lg border border-border/60 px-3 py-2 text-sm text-muted-foreground">
                    {definition.notes}
                  </p>
                )}
                {definition.source && (
                  <p className="mt-2 text-xs text-muted-foreground">
                    {t("providerCenter.sourceImported", {
                      app: appLabel(definition.source.sourceApp),
                      time: formatTime(definition.source.importedAt),
                    })}
                  </p>
                )}

                <div className="mt-4 space-y-2 border-t border-border/60 pt-4">
                  <div className="flex items-center justify-between">
                    <p className="text-sm font-medium">
                      {t("providerCenter.bindingsTitle")}
                    </p>
                    <span className="text-xs text-muted-foreground">
                      {t("providerCenter.bindingsHint")}
                    </span>
                  </div>
                  {definitionBindings.length === 0 ? (
                    <p className="rounded-lg bg-muted/30 px-3 py-2 text-sm text-muted-foreground">
                      {t("providerCenter.bindingsEmpty")}
                    </p>
                  ) : (
                    definitionBindings.map((binding) => (
                      <div
                        key={binding.appType}
                        className="flex flex-col gap-2 rounded-xl border border-border/60 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between"
                      >
                        <div className="min-w-0">
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="text-sm font-medium">
                              {appLabel(binding.appType)}
                            </span>
                            <Badge
                              variant="outline"
                              className={statusClass[binding.status]}
                            >
                              {t(`providerCenter.status.${binding.status}`)}
                            </Badge>
                          </div>
                          {binding.lastError && (
                            <p className="mt-1 text-xs text-destructive">
                              {binding.lastError}
                            </p>
                          )}
                        </div>
                        {binding.enabled &&
                          binding.status !== "unsupported" && (
                            <div className="flex shrink-0 items-center gap-2">
                              <label className="flex items-center gap-2 text-xs text-muted-foreground">
                                <Switch
                                  checked={binding.overrideEnabled}
                                  onCheckedChange={(checked) =>
                                    void updateOverride(binding, checked)
                                  }
                                />
                                {t("providerCenter.overrideLabel")}
                              </label>
                              <Button
                                size="sm"
                                variant="ghost"
                                onClick={() => void detachBinding(binding)}
                              >
                                <Unlink className="mr-1.5 h-3.5 w-3.5" />
                                {t("providerCenter.detach")}
                              </Button>
                            </div>
                          )}
                      </div>
                    ))
                  )}
                </div>

                <div className="mt-4 flex flex-wrap gap-2">
                  <Button
                    size="sm"
                    onClick={() => void prepareApply(definition)}
                    disabled={busy || !definition.enabled}
                  >
                    <Link2 className="mr-2 h-4 w-4" />
                    {t("providerCenter.previewApply")}
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => void discoverModels(definition)}
                    disabled={
                      busy ||
                      (definition.protocol !== "ollama" &&
                        !definition.credentialConfigured)
                    }
                  >
                    <RefreshCw className="mr-2 h-4 w-4" />
                    {t("providerCenter.discover")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => openEdit(definition)}
                  >
                    <Pencil className="mr-2 h-4 w-4" />
                    {t("providerCenter.edit")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => void duplicate(definition)}
                  >
                    <Copy className="mr-2 h-4 w-4" />
                    {t("providerCenter.duplicate")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="text-destructive hover:text-destructive"
                    onClick={() => setDeleteTarget(definition)}
                  >
                    <Trash2 className="mr-2 h-4 w-4" />
                    {t("providerCenter.delete")}
                  </Button>
                </div>
                {definition.lastDiscoveryError && (
                  <p className="mt-2 text-xs text-destructive">
                    {t("providerCenter.lastDiscoverError", {
                      error: definition.lastDiscoveryError,
                    })}
                  </p>
                )}
              </article>
            );
          })}
        </div>
      )}

      <Dialog open={formOpen} onOpenChange={setFormOpen}>
        <DialogContent className="max-h-[90vh] max-w-2xl overflow-y-auto">
          <DialogHeader>
            <DialogTitle>
              {form.id
                ? t("providerCenter.form.editTitle")
                : t("providerCenter.form.addTitle")}
            </DialogTitle>
            <DialogDescription>
              {t("providerCenter.form.description")}
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-4 py-2">
            <div className="grid gap-2 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="provider-name">
                  {t("providerCenter.form.name")}
                </Label>
                <Input
                  id="provider-name"
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
                <Label>{t("providerCenter.form.protocol")}</Label>
                <Select
                  value={form.protocol}
                  onValueChange={(protocol) =>
                    setForm((current) => ({ ...current, protocol }))
                  }
                >
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {PROTOCOLS.map((protocol) => (
                      <SelectItem key={protocol.id} value={protocol.id}>
                        {protocol.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>
            <div className="space-y-2">
              <Label htmlFor="provider-url">
                {t("providerCenter.form.baseUrl")}
              </Label>
              <Input
                id="provider-url"
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
            <div className="space-y-2">
              <Label>API Key</Label>
              {form.protocol === "ollama" && (
                <p className="text-xs text-muted-foreground">
                  {t("providerCenter.form.ollamaNoKey")}
                </p>
              )}
              {form.protocol !== "ollama" && (
                <>
                  {form.id && (
                    <div className="flex flex-wrap gap-2">
                      <Button
                        type="button"
                        size="sm"
                        variant={
                          form.credentialAction === "keep"
                            ? "default"
                            : "outline"
                        }
                        onClick={() =>
                          setForm((current) => ({
                            ...current,
                            credentialAction: "keep",
                            apiKey: "",
                          }))
                        }
                      >
                        {t("providerCenter.form.credKeep")}
                      </Button>
                      <Button
                        type="button"
                        size="sm"
                        variant={
                          form.credentialAction === "replace"
                            ? "default"
                            : "outline"
                        }
                        onClick={() =>
                          setForm((current) => ({
                            ...current,
                            credentialAction: "replace",
                          }))
                        }
                      >
                        {t("providerCenter.form.credReplace")}
                      </Button>
                      <Button
                        type="button"
                        size="sm"
                        variant={
                          form.credentialAction === "clear"
                            ? "destructive"
                            : "outline"
                        }
                        onClick={() =>
                          setForm((current) => ({
                            ...current,
                            credentialAction: "clear",
                            apiKey: "",
                          }))
                        }
                      >
                        {t("providerCenter.form.credClear")}
                      </Button>
                    </div>
                  )}
                  {form.credentialAction === "replace" && (
                    <Input
                      type="password"
                      autoComplete="new-password"
                      value={form.apiKey}
                      onChange={(event) =>
                        setForm((current) => ({
                          ...current,
                          apiKey: event.target.value,
                        }))
                      }
                      placeholder={t(
                        "providerCenter.form.credReplacePlaceholder",
                      )}
                    />
                  )}
                  {form.credentialAction === "keep" && (
                    <p className="text-xs text-muted-foreground">
                      {t("providerCenter.form.credKeepHint")}
                    </p>
                  )}
                  {form.credentialAction === "clear" && (
                    <p className="text-xs text-destructive">
                      {t("providerCenter.form.credClearHint")}
                    </p>
                  )}
                </>
              )}
            </div>
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <Label htmlFor="provider-models">
                  {t("providerCenter.form.models")}
                </Label>
                <span className="text-xs text-muted-foreground">
                  {t("providerCenter.form.modelsHint")}
                </span>
              </div>
              <Textarea
                id="provider-models"
                value={form.models}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    models: event.target.value,
                  }))
                }
                placeholder={"gpt-5\ngpt-5-mini"}
                rows={4}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="provider-notes">
                {t("providerCenter.form.notes")}
              </Label>
              <Textarea
                id="provider-notes"
                value={form.notes}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    notes: event.target.value,
                  }))
                }
                placeholder={t("providerCenter.form.notesPlaceholder")}
                rows={2}
              />
            </div>
            <div className="space-y-2">
              <div>
                <Label>{t("providerCenter.form.apps")}</Label>
                <p className="mt-1 text-xs text-muted-foreground">
                  {t("providerCenter.form.appsHint")}
                </p>
              </div>
              <AppChooser
                apps={availableApps}
                value={form.appTypes}
                onChange={(appTypes) =>
                  setForm((current) => ({ ...current, appTypes }))
                }
              />
            </div>
            <label className="flex items-center justify-between rounded-xl border border-border/70 px-3 py-3">
              <div>
                <p className="text-sm font-medium">
                  {t("providerCenter.form.enabled")}
                </p>
                <p className="text-xs text-muted-foreground">
                  {t("providerCenter.form.enabledHint")}
                </p>
              </div>
              <Switch
                checked={form.enabled}
                onCheckedChange={(enabled) =>
                  setForm((current) => ({ ...current, enabled }))
                }
              />
            </label>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setFormOpen(false)}>
              {t("providerCenter.form.cancel")}
            </Button>
            <Button onClick={() => void save()} disabled={busy}>
              {busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {t("providerCenter.form.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={importsOpen} onOpenChange={setImportsOpen}>
        <DialogContent className="max-h-[90vh] max-w-3xl overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{t("providerCenter.import.title")}</DialogTitle>
            <DialogDescription>
              {t("providerCenter.import.description", {
                total: candidates.length,
                withCredentials: importSummary,
              })}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3 py-2">
            {importErrors.length > 0 && (
              <div className="rounded-xl border border-amber-500/30 bg-amber-500/5 p-3 text-sm text-amber-800 dark:text-amber-200">
                {t("providerCenter.import.partialErrors")}
                {importErrors
                  .map((error) =>
                    t("providerCenter.import.errorItem", {
                      app: appLabel(error.appType),
                      message: error.message,
                    }),
                  )
                  .join(t("providerCenter.import.errorSeparator"))}
              </div>
            )}
            {candidates.length === 0 ? (
              <div className="rounded-xl border border-dashed p-8 text-center text-sm text-muted-foreground">
                {t("providerCenter.import.empty")}
              </div>
            ) : (
              candidates.map((candidate) => (
                <section
                  key={candidate.sourceRef}
                  className="rounded-xl border border-border/70 p-4"
                >
                  <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <h4 className="font-medium">{candidate.name}</h4>
                        <Badge variant="outline">
                          {appLabel(candidate.sourceApp)}
                        </Badge>
                        {candidate.credentialConfigured ? (
                          <Badge className="border-emerald-500/30 bg-emerald-500/10 text-emerald-700 hover:bg-emerald-500/10 dark:text-emerald-300">
                            {t("providerCenter.import.hasCredential")}
                          </Badge>
                        ) : (
                          <Badge variant="secondary">
                            {t("providerCenter.import.noCredential")}
                          </Badge>
                        )}
                      </div>
                      <p className="mt-1 break-all text-sm text-muted-foreground">
                        {candidate.baseUrl}
                      </p>
                      {candidate.credentialHint && (
                        <p className="mt-1 text-xs text-muted-foreground">
                          {t("providerCenter.import.credentialHint", {
                            hint: candidate.credentialHint,
                          })}
                        </p>
                      )}
                    </div>
                    <Button
                      size="sm"
                      onClick={() => void importCandidate(candidate)}
                      disabled={
                        busy ||
                        (importApps[candidate.sourceRef]?.length ?? 0) === 0
                      }
                    >
                      <Download className="mr-2 h-4 w-4" />
                      {t("providerCenter.import.copy")}
                    </Button>
                  </div>
                  {candidate.models.length > 0 && (
                    <div className="mt-3 flex flex-wrap gap-1.5">
                      {candidate.models.map((model) => (
                        <Badge
                          key={model}
                          variant="secondary"
                          className="font-normal"
                        >
                          {model}
                        </Badge>
                      ))}
                    </div>
                  )}
                  <div className="mt-4 space-y-2">
                    <p className="text-xs font-medium text-muted-foreground">
                      {t("providerCenter.import.bindTo")}
                    </p>
                    <AppChooser
                      apps={availableApps}
                      value={importApps[candidate.sourceRef] ?? []}
                      onChange={(appTypes) =>
                        setImportApps((current) => ({
                          ...current,
                          [candidate.sourceRef]: appTypes,
                        }))
                      }
                    />
                  </div>
                </section>
              ))
            )}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setImportsOpen(false)}>
              {t("providerCenter.import.close")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={Boolean(preview)}
        onOpenChange={(open) => {
          if (!open) {
            setPreview(null);
            setPreviewDefinition(null);
          }
        }}
      >
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>
              {t("providerCenter.preview.title", {
                name: previewDefinition?.name,
              })}
            </DialogTitle>
            <DialogDescription>
              {t("providerCenter.preview.description")}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3 py-2">
            {preview?.targets.map((target) => (
              <div
                key={target.appType}
                className="flex items-start justify-between gap-3 rounded-xl border border-border/70 p-3"
              >
                <div>
                  <p className="text-sm font-medium">
                    {appLabel(target.appType)}
                  </p>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {target.message ||
                      (target.operation === "create"
                        ? t("providerCenter.preview.opCreate")
                        : target.operation === "update"
                          ? t("providerCenter.preview.opUpdate")
                          : t("providerCenter.preview.opUnsupported"))}
                  </p>
                </div>
                {!target.compatible ? (
                  <Badge variant="destructive">
                    {t("providerCenter.preview.incompatible")}
                  </Badge>
                ) : target.drifted ? (
                  <Badge className="border-orange-500/30 bg-orange-500/10 text-orange-700 hover:bg-orange-500/10 dark:text-orange-300">
                    {t("providerCenter.preview.drifted")}
                  </Badge>
                ) : (
                  <Badge className="border-emerald-500/30 bg-emerald-500/10 text-emerald-700 hover:bg-emerald-500/10 dark:text-emerald-300">
                    {t("providerCenter.preview.compatible")}
                  </Badge>
                )}
              </div>
            ))}
            {previewBlocked && (
              <div className="flex gap-2 rounded-xl border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
                <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
                <p>{t("providerCenter.preview.blockedWarning")}</p>
              </div>
            )}
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => {
                setPreview(null);
                setPreviewDefinition(null);
              }}
            >
              {t("providerCenter.preview.cancel")}
            </Button>
            <Button
              onClick={() => void confirmApply()}
              disabled={busy || previewBlocked}
            >
              {busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {t("providerCenter.preview.confirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={Boolean(transactionResult)}
        onOpenChange={(open) => {
          if (!open) setTransactionResult(null);
        }}
      >
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>{t("providerCenter.result.title")}</DialogTitle>
            <DialogDescription>
              {t("providerCenter.result.description")}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3 py-2">
            <div className="flex items-center gap-2 rounded-xl bg-muted/40 p-3">
              {transactionResult?.status === "applied" ||
              transactionResult?.status === "restored" ? (
                <CheckCircle2 className="h-5 w-5 text-emerald-600" />
              ) : (
                <AlertTriangle className="h-5 w-5 text-destructive" />
              )}
              <div>
                <p className="text-sm font-medium">
                  {t("providerCenter.result.status", {
                    status: transactionResult?.status,
                  })}
                </p>
                <p className="text-xs text-muted-foreground">
                  {t("providerCenter.result.transaction", {
                    id: transactionResult?.id,
                    time: formatTime(
                      transactionResult?.completedAt ??
                        transactionResult?.createdAt,
                    ),
                  })}
                </p>
              </div>
            </div>
            {transactionResult?.targets.map((target) => (
              <div
                key={target.appType}
                className="flex items-start justify-between gap-3 rounded-xl border border-border/70 p-3"
              >
                <div>
                  <p className="text-sm font-medium">
                    {appLabel(target.appType)}
                  </p>
                  {target.message && (
                    <p className="mt-1 text-xs text-muted-foreground">
                      {target.message}
                    </p>
                  )}
                </div>
                <Badge
                  variant={
                    target.status === "applied" ||
                    target.status === "restored" ||
                    target.status === "rolled_back"
                      ? "secondary"
                      : "destructive"
                  }
                >
                  {target.status}
                </Badge>
              </div>
            ))}
            {transactionResult?.status === "recoveryRequired" && (
              <div className="flex gap-2 rounded-xl border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
                <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
                {t("providerCenter.result.recoveryRequiredWarning")}
              </div>
            )}
          </div>
          <DialogFooter>
            {transactionResult &&
              transactionResult.status !== "applied" &&
              transactionResult.status !== "restored" && (
                <Button
                  variant="outline"
                  onClick={() => void restore(transactionResult)}
                  disabled={busy}
                >
                  <RotateCcw className="mr-2 h-4 w-4" />
                  {t("providerCenter.result.tryRestore")}
                </Button>
              )}
            <Button onClick={() => setTransactionResult(null)}>
              {t("providerCenter.result.done")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={historyOpen} onOpenChange={setHistoryOpen}>
        <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{t("providerCenter.history")}</DialogTitle>
            <DialogDescription>
              {t("providerCenter.historyDialog.description")}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3 py-2">
            {transactions.length === 0 ? (
              <p className="rounded-xl border border-dashed p-8 text-center text-sm text-muted-foreground">
                {t("providerCenter.historyDialog.empty")}
              </p>
            ) : (
              transactions.map((transaction) => {
                const definition = definitions.find(
                  (item) => item.id === transaction.providerId,
                );
                return (
                  <div
                    key={transaction.id}
                    className="rounded-xl border border-border/70 p-3"
                  >
                    <div className="flex items-start justify-between gap-3">
                      <div>
                        <p className="text-sm font-medium">
                          {definition?.name ??
                            t("providerCenter.historyDialog.deletedService")}
                        </p>
                        <p className="mt-1 text-xs text-muted-foreground">
                          {formatTime(
                            transaction.completedAt ?? transaction.createdAt,
                          )}{" "}
                          ·{" "}
                          {transaction.targets
                            .map((target) => appLabel(target.appType))
                            .join(t("providerCenter.listSeparator"))}
                        </p>
                      </div>
                      <Badge
                        variant={
                          transaction.status === "applied" ||
                          transaction.status === "restored"
                            ? "secondary"
                            : "destructive"
                        }
                      >
                        {transaction.status}
                      </Badge>
                    </div>
                    <div className="mt-3 flex justify-end">
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => void restore(transaction)}
                        disabled={busy || transaction.status === "restored"}
                      >
                        <RotateCcw className="mr-2 h-4 w-4" />
                        {t("providerCenter.historyDialog.restoreRecord")}
                      </Button>
                    </div>
                  </div>
                );
              })
            )}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setHistoryOpen(false)}>
              {t("providerCenter.historyDialog.close")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={Boolean(deleteTarget)}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {t("providerCenter.deleteDialog.title", {
                name: deleteTarget?.name,
              })}
            </DialogTitle>
            <DialogDescription>
              {t("providerCenter.deleteDialog.description")}
            </DialogDescription>
          </DialogHeader>
          <div className="flex gap-2 rounded-xl border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
            <Unlink className="mt-0.5 h-4 w-4 shrink-0" />
            {t("providerCenter.deleteDialog.warning")}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDeleteTarget(null)}>
              {t("providerCenter.deleteDialog.cancel")}
            </Button>
            <Button
              variant="destructive"
              onClick={() => void remove()}
              disabled={busy}
            >
              {busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {t("providerCenter.deleteDialog.confirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
