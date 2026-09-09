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

const statusText: Record<ProviderBinding["status"], string> = {
  pending: "待应用",
  applied: "已接入",
  overridden: "已单独自定义",
  unsupported: "暂不兼容",
  drifted: "配置已变化",
  detached: "已解除关联",
  failed: "应用失败",
};

const statusClass: Record<ProviderBinding["status"], string> = {
  pending: "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  applied: "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  overridden: "border-blue-500/30 bg-blue-500/10 text-blue-700 dark:text-blue-300",
  unsupported: "border-muted-foreground/20 bg-muted text-muted-foreground",
  drifted: "border-orange-500/30 bg-orange-500/10 text-orange-700 dark:text-orange-300",
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
  return Array.from(new Set(value.split(/[\n,]/).map((item) => item.trim()).filter(Boolean)));
}

function appLabel(appType: string): string {
  return APPS.find((app) => app.id === appType)?.label ?? appType;
}

function formatTime(value?: number): string {
  if (!value) return "—";
  // Provider Center timestamps are milliseconds since Unix epoch.
  return new Date(value).toLocaleString();
}

function AppChooser({ apps, value, onChange }: {
  apps: readonly { id: string; label: string }[];
  value: string[];
  onChange: (apps: string[]) => void;
}) {
  return <div className="grid gap-2 sm:grid-cols-2">{apps.map((app) => {
    const checked = value.includes(app.id);
    return <label key={app.id} className="flex cursor-pointer items-center gap-2 rounded-lg border border-border/70 px-3 py-2 text-sm transition-colors hover:bg-muted/50">
      <Checkbox checked={checked} onCheckedChange={(next) => onChange(next === true ? [...value, app.id] : value.filter((id) => id !== app.id))} />
      {app.label}
    </label>;
  })}</div>;
}

export function ProviderCenterPanel() {
  const [definitions, setDefinitions] = useState<ProviderDefinition[]>([]);
  const [bindings, setBindings] = useState<ProviderBinding[]>([]);
  const [transactions, setTransactions] = useState<ProviderApplyTransaction[]>([]);
  const [catalog, setCatalog] = useState<UnifiedModelCatalogEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [formOpen, setFormOpen] = useState(false);
  const [form, setForm] = useState<ProviderFormState>(() => emptyForm(["codex"]));
  const [importsOpen, setImportsOpen] = useState(false);
  const [importSessionId, setImportSessionId] = useState<string | null>(null);
  const [importErrors, setImportErrors] = useState<string[]>([]);
  const [candidates, setCandidates] = useState<ImportCandidate[]>([]);
  const [importApps, setImportApps] = useState<Record<string, string[]>>({});
  const [preview, setPreview] = useState<ProviderApplyPreview | null>(null);
  const [previewDefinition, setPreviewDefinition] = useState<ProviderDefinition | null>(null);
  const [transactionResult, setTransactionResult] = useState<ProviderApplyTransaction | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<ProviderDefinition | null>(null);
  const [availableApps, setAvailableApps] = useState<readonly { id: string; label: string }[]>(APPS);

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
      toast.error(`加载模型服务失败：${String(error)}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void load(); }, [load]);
  useEffect(() => {
    const toolApps = APPS.filter((app) => "tool" in app);
    void Promise.all([
      settingsApi.getToolVersions(toolApps.map((app) => app.tool)),
      settingsApi.getDesktopAppStatus("codex-desktop").catch(() => null),
      settingsApi.getDesktopAppStatus("claude-desktop").catch(() => null),
    ]).then(([tools, codexDesktop, claudeDesktop]) => {
      const installedTools = new Set(tools.filter((tool) => Boolean(tool.version)).map((tool) => tool.name === "grok" ? "grokbuild" : tool.name));
      const detected = APPS.filter((app) => {
        if (app.id === "claude-desktop") return Boolean(claudeDesktop?.installed);
        if (app.id === "codex" && codexDesktop?.installed) return true;
        return installedTools.has(app.id);
      });
      if (detected.length > 0) {
        setAvailableApps(detected);
        setForm((current) => ({ ...current, appTypes: current.appTypes.filter((app) => detected.some((item) => item.id === app)) }));
      }
    }).catch(() => {
      // 浏览器预览没有完整桌面探测能力；保留完整列表用于界面预览。
    });
  }, []);

  const bindingsFor = useCallback((id: string) => bindings.filter((binding) => binding.providerId === id), [bindings]);
  const importSummary = useMemo(() => candidates.filter((candidate) => candidate.credentialConfigured).length, [candidates]);

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
      appTypes: bindingsFor(definition.id).filter((binding) => binding.enabled).map((binding) => binding.appType),
    });
    setFormOpen(true);
  };

  const save = async () => {
    if (!form.name.trim() || !form.baseUrl.trim()) {
      toast.error("请填写名称和请求地址");
      return;
    }
    if (form.protocol !== "ollama" && form.credentialAction === "replace" && !form.apiKey.trim()) {
      toast.error("请输入新的 API Key");
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
      credentialAction: form.protocol === "ollama" ? "clear" : form.credentialAction,
      apiKey: form.protocol !== "ollama" && form.credentialAction === "replace" ? form.apiKey : undefined,
      appTypes: form.appTypes,
    };
    setBusy(true);
    try {
      await providerCenterApi.save(input);
      setFormOpen(false);
      await load();
      toast.success(form.id ? "模型服务已更新，修改等待应用" : "模型服务已安全保存");
    } catch (error) {
      toast.error(`保存失败：${String(error)}`);
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
      setImportApps(Object.fromEntries(session.candidates.map((candidate) => [candidate.sourceRef, availableApps[0] ? [availableApps[0].id] : []])));
      setImportsOpen(true);
    } catch (error) {
      toast.error(`扫描本机配置失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const importCandidate = async (candidate: ImportCandidate) => {
    setBusy(true);
    try {
      if (importSessionId && candidate.id) {
        await providerCenterApi.commitImportCandidate(
          importSessionId,
          candidate.id,
          importApps[candidate.sourceRef] ?? [],
        );
        const session = await providerCenterApi.getImportSession(importSessionId);
        setCandidates(session.candidates);
      } else {
        await providerCenterApi.importCandidate(candidate.sourceRef, importApps[candidate.sourceRef] ?? []);
      }
      await load();
      toast.success(`已复制“${candidate.name}”，来源配置没有被修改`);
    } catch (error) {
      toast.error(`导入失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const discoverModels = async (definition: ProviderDefinition) => {
    setBusy(true);
    try {
      const result = await providerCenterApi.discoverModels(definition.id);
      await load();
      if (result.error) toast.error("模型获取失败", { description: result.error });
      else toast.success(`已发现 ${result.models.length} 个模型`);
    } catch (error) {
      toast.error(`模型获取失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const duplicate = async (definition: ProviderDefinition) => {
    setBusy(true);
    try {
      await providerCenterApi.duplicate(definition.id);
      await load();
      toast.success("已复制模型服务，密钥仍只保存在安全存储中");
    } catch (error) {
      toast.error(`复制失败：${String(error)}`);
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
      toast.success("模型服务已从共享中心删除；原应用配置未被静默删除");
    } catch (error) {
      toast.error(`删除失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const toggleEnabled = async (definition: ProviderDefinition, enabled: boolean) => {
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
        appTypes: bindingsFor(definition.id).filter((binding) => binding.enabled).map((binding) => binding.appType),
      });
      await load();
      toast.success(enabled ? "模型服务已启用，应用前仍需确认" : "模型服务已停用");
    } catch (error) {
      toast.error(`操作失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const updateOverride = async (binding: ProviderBinding, enabled: boolean) => {
    try {
      await providerCenterApi.setOverride(binding.providerId, binding.appType, enabled);
      await load();
      toast.success(enabled ? "该应用已保留单独配置" : "该应用已恢复共享配置，等待重新应用");
    } catch (error) {
      toast.error(`更新失败：${String(error)}`);
    }
  };

  const prepareApply = async (definition: ProviderDefinition) => {
    const targets = bindingsFor(definition.id).filter((binding) => binding.enabled && !binding.overrideEnabled).map((binding) => binding.appType);
    if (targets.length === 0) {
      toast.error("没有可应用的目标，请先编辑并选择应用");
      return;
    }
    setBusy(true);
    try {
      const next = await providerCenterApi.previewApply(definition.id, targets);
      setPreviewDefinition(definition);
      setPreview(next);
    } catch (error) {
      toast.error(`生成应用预览失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const confirmApply = async () => {
    if (!preview || !previewDefinition) return;
    setBusy(true);
    try {
      const result = await providerCenterApi.applyTransaction(previewDefinition.id, preview.targets.map((target) => target.appType), preview.token);
      setPreview(null);
      setPreviewDefinition(null);
      setTransactionResult(result);
      await load();
      if (result.status === "applied") toast.success("共享配置已应用并完成校验");
      else toast.error("部分应用未成功，已执行回滚或需要恢复");
    } catch (error) {
      toast.error(`应用失败：${String(error)}`);
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
      if (result.status === "restored") toast.success("已恢复到应用前的配置");
      else toast.error("自动恢复未完全成功，请查看逐应用结果");
    } catch (error) {
      toast.error(`恢复失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  if (loading) {
    return <div className="flex min-h-72 items-center justify-center text-muted-foreground"><Loader2 className="mr-2 h-4 w-4 animate-spin" />正在加载模型服务…</div>;
  }

  const previewBlocked = preview?.targets.some((target) => !target.compatible || target.drifted) ?? false;
  const enabledCount = definitions.filter((definition) => definition.enabled).length;
  const pendingCount = bindings.filter((binding) => binding.enabled && ["pending", "drifted", "failed"].includes(binding.status)).length;
  const recoveryRequiredTransactions = transactions.filter((transaction) => transaction.status === "recoveryRequired");

  return (
    <div className="space-y-5">
      {recoveryRequiredTransactions.length > 0 && <section className="flex items-start justify-between gap-4 rounded-2xl border border-destructive/40 bg-destructive/5 p-4 text-sm text-destructive">
        <div className="flex gap-2"><AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" /><div><p className="font-medium">有 {recoveryRequiredTransactions.length} 次配置操作需要恢复</p><p className="mt-1 text-xs opacity-80">上次自动恢复未完全成功。请打开应用历史，查看失败应用并再次恢复。</p></div></div>
        <Button size="sm" variant="outline" onClick={() => setHistoryOpen(true)}>打开应用历史</Button>
      </section>}
      <section className="rounded-2xl border border-border/70 bg-card p-5 shadow-sm">
        <div className="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <div className="space-y-2">
            <div className="flex items-center gap-2">
              <div className="rounded-xl bg-primary/10 p-2 text-primary"><CloudCog className="h-5 w-5" /></div>
              <div>
                <h2 className="text-xl font-semibold">共享模型服务</h2>
                <p className="text-sm text-muted-foreground">集中保存模型服务，再明确选择要接入的应用。</p>
              </div>
            </div>
            <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
              <Badge variant="secondary">{definitions.length} 个服务</Badge>
              <Badge variant="secondary">{enabledCount} 个已启用</Badge>
              {pendingCount > 0 && <Badge className="border-amber-500/30 bg-amber-500/10 text-amber-700 hover:bg-amber-500/10 dark:text-amber-300">{pendingCount} 项待处理</Badge>}
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" onClick={() => void openImports()} disabled={busy}><Download className="mr-2 h-4 w-4" />导入本机配置</Button>
            <Button variant="outline" onClick={() => setHistoryOpen(true)} disabled={transactions.length === 0}><History className="mr-2 h-4 w-4" />应用历史</Button>
            <Button onClick={openCreate}><Plus className="mr-2 h-4 w-4" />添加模型服务</Button>
          </div>
        </div>
        <div className="mt-4 flex gap-2 rounded-xl border border-blue-500/20 bg-blue-500/5 px-3 py-2 text-sm text-muted-foreground">
          <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-blue-600" />
          <p>API Key 只保存在系统安全存储中，界面不会读取明文。保存只更新共享定义和关联状态，点击“预览并应用”后才会修改所选应用。</p>
        </div>
      </section>

      {catalog.length > 0 && <section className="rounded-2xl border border-border/70 bg-card p-5 shadow-sm">
        <div className="flex items-center gap-2"><Sparkles className="h-4 w-4 text-primary" /><div><h3 className="font-medium">当前可用模型</h3><p className="text-xs text-muted-foreground">只显示已成功接入、配置未漂移且当前应用能够使用的模型；原生账号与 API Key 分开标记。</p></div></div>
        <div className="mt-4 grid gap-2 sm:grid-cols-2 xl:grid-cols-3">{catalog.map((entry) => <div key={entry.id} className="rounded-xl border border-border/60 px-3 py-2.5"><div className="flex items-center justify-between gap-2"><p className="truncate text-sm font-medium">{entry.modelId}</p><Badge variant="outline">{appLabel(entry.appType)}</Badge></div><p className="mt-1 truncate text-xs text-muted-foreground">{entry.providerName} · {entry.sourceType === "nativeAccount" ? "原生账号" : "API Key"}{entry.isDefault ? " · 默认" : ""}</p></div>)}</div>
      </section>}

      {definitions.length === 0 ? (
        <section className="flex min-h-64 flex-col items-center justify-center rounded-2xl border border-dashed border-border bg-muted/20 px-6 text-center">
          <CloudCog className="mb-3 h-10 w-10 text-muted-foreground/60" />
          <h3 className="font-medium">还没有共享模型服务</h3>
          <p className="mt-1 max-w-lg text-sm text-muted-foreground">可以手动添加，也可以扫描电脑上已有的配置。导入只复制内容，不会修改来源应用。</p>
          <div className="mt-4 flex gap-2">
            <Button variant="outline" onClick={() => void openImports()}><Search className="mr-2 h-4 w-4" />扫描本机配置</Button>
            <Button onClick={openCreate}><Plus className="mr-2 h-4 w-4" />添加模型服务</Button>
          </div>
        </section>
      ) : (
        <div className="grid gap-4 xl:grid-cols-2">
          {definitions.map((definition) => {
            const definitionBindings = bindingsFor(definition.id);
            const allModels = Array.from(new Set([...definition.models, ...definition.discoveredModels]));
            return (
              <article key={definition.id} className="rounded-2xl border border-border/70 bg-card p-5 shadow-sm">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <h3 className="truncate text-lg font-semibold">{definition.name}</h3>
                      <Badge variant="outline">{PROTOCOLS.find((protocol) => protocol.id === definition.protocol)?.label ?? definition.protocol}</Badge>
                      {!definition.enabled && <Badge variant="secondary">已停用</Badge>}
                    </div>
                    <p className="mt-1 break-all text-sm text-muted-foreground">{definition.baseUrl}</p>
                  </div>
                  <Switch checked={definition.enabled} onCheckedChange={(checked) => void toggleEnabled(definition, checked)} aria-label={`启用 ${definition.name}`} />
                </div>

                <div className="mt-4 grid gap-3 sm:grid-cols-2">
                  <div className="rounded-xl bg-muted/40 p-3">
                    <div className="flex items-center gap-2 text-xs text-muted-foreground"><KeyRound className="h-3.5 w-3.5" />密钥</div>
                    <p className="mt-1 text-sm font-medium">{definition.credentialConfigured ? `已安全保存${definition.credentialHint ? ` · ${definition.credentialHint}` : ""}` : "未配置"}</p>
                  </div>
                  <div className="rounded-xl bg-muted/40 p-3">
                    <div className="text-xs text-muted-foreground">模型</div>
                    <p className="mt-1 text-sm font-medium">{allModels.length > 0 ? `${allModels.length} 个可选模型` : "尚未添加模型"}</p>
                  </div>
                </div>

                {allModels.length > 0 && <div className="mt-3 flex max-h-20 flex-wrap gap-1.5 overflow-y-auto">{allModels.map((model) => <Badge key={model} variant="secondary" className="font-normal">{model}</Badge>)}</div>}
                {definition.notes && <p className="mt-3 rounded-lg border border-border/60 px-3 py-2 text-sm text-muted-foreground">{definition.notes}</p>}
                {definition.source && <p className="mt-2 text-xs text-muted-foreground">来源：从 {appLabel(definition.source.sourceApp)} 导入 · {formatTime(definition.source.importedAt)}</p>}

                <div className="mt-4 space-y-2 border-t border-border/60 pt-4">
                  <div className="flex items-center justify-between"><p className="text-sm font-medium">接入应用</p><span className="text-xs text-muted-foreground">修改后需显式应用</span></div>
                  {definitionBindings.length === 0 ? <p className="rounded-lg bg-muted/30 px-3 py-2 text-sm text-muted-foreground">尚未选择应用，请编辑此服务。</p> : definitionBindings.map((binding) => (
                    <div key={binding.appType} className="flex flex-col gap-2 rounded-xl border border-border/60 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between">
                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-2"><span className="text-sm font-medium">{appLabel(binding.appType)}</span><Badge variant="outline" className={statusClass[binding.status]}>{statusText[binding.status]}</Badge></div>
                        {binding.lastError && <p className="mt-1 text-xs text-destructive">{binding.lastError}</p>}
                      </div>
                      {binding.enabled && binding.status !== "unsupported" && <label className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground"><Switch checked={binding.overrideEnabled} onCheckedChange={(checked) => void updateOverride(binding, checked)} />单独自定义</label>}
                    </div>
                  ))}
                </div>

                <div className="mt-4 flex flex-wrap gap-2">
                  <Button size="sm" onClick={() => void prepareApply(definition)} disabled={busy || !definition.enabled}><Link2 className="mr-2 h-4 w-4" />预览并应用</Button>
                  <Button size="sm" variant="outline" onClick={() => void discoverModels(definition)} disabled={busy || (definition.protocol !== "ollama" && !definition.credentialConfigured)}><RefreshCw className="mr-2 h-4 w-4" />获取模型</Button>
                  <Button size="sm" variant="ghost" onClick={() => openEdit(definition)}><Pencil className="mr-2 h-4 w-4" />编辑</Button>
                  <Button size="sm" variant="ghost" onClick={() => void duplicate(definition)}><Copy className="mr-2 h-4 w-4" />复制</Button>
                  <Button size="sm" variant="ghost" className="text-destructive hover:text-destructive" onClick={() => setDeleteTarget(definition)}><Trash2 className="mr-2 h-4 w-4" />删除</Button>
                </div>
                {definition.lastDiscoveryError && <p className="mt-2 text-xs text-destructive">上次获取模型失败：{definition.lastDiscoveryError}</p>}
              </article>
            );
          })}
        </div>
      )}

      <Dialog open={formOpen} onOpenChange={setFormOpen}>
        <DialogContent className="max-h-[90vh] max-w-2xl overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{form.id ? "编辑共享模型服务" : "添加共享模型服务"}</DialogTitle>
            <DialogDescription>保存后只更新共享定义和关联状态，不会立即改写任何应用配置。</DialogDescription>
          </DialogHeader>
          <div className="grid gap-4 py-2">
            <div className="grid gap-2 sm:grid-cols-2">
              <div className="space-y-2"><Label htmlFor="provider-name">名称</Label><Input id="provider-name" value={form.name} onChange={(event) => setForm((current) => ({ ...current, name: event.target.value }))} placeholder="例如：公司 OpenAI 网关" /></div>
              <div className="space-y-2"><Label>接口协议</Label><Select value={form.protocol} onValueChange={(protocol) => setForm((current) => ({ ...current, protocol }))}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent>{PROTOCOLS.map((protocol) => <SelectItem key={protocol.id} value={protocol.id}>{protocol.label}</SelectItem>)}</SelectContent></Select></div>
            </div>
            <div className="space-y-2"><Label htmlFor="provider-url">请求地址</Label><Input id="provider-url" value={form.baseUrl} onChange={(event) => setForm((current) => ({ ...current, baseUrl: event.target.value }))} placeholder="https://api.example.com/v1" /></div>
            <div className="space-y-2">
              <Label>API Key</Label>
              {form.protocol === "ollama" && <p className="text-xs text-muted-foreground">本机 Ollama 默认不需要 API Key。</p>}
              {form.protocol !== "ollama" && <>
              {form.id && <div className="flex flex-wrap gap-2"><Button type="button" size="sm" variant={form.credentialAction === "keep" ? "default" : "outline"} onClick={() => setForm((current) => ({ ...current, credentialAction: "keep", apiKey: "" }))}>保留现有密钥</Button><Button type="button" size="sm" variant={form.credentialAction === "replace" ? "default" : "outline"} onClick={() => setForm((current) => ({ ...current, credentialAction: "replace" }))}>替换密钥</Button><Button type="button" size="sm" variant={form.credentialAction === "clear" ? "destructive" : "outline"} onClick={() => setForm((current) => ({ ...current, credentialAction: "clear", apiKey: "" }))}>清除密钥</Button></div>}
              {form.credentialAction === "replace" && <Input type="password" autoComplete="new-password" value={form.apiKey} onChange={(event) => setForm((current) => ({ ...current, apiKey: event.target.value }))} placeholder="密钥只会写入系统安全存储" />}
              {form.credentialAction === "keep" && <p className="text-xs text-muted-foreground">现有密钥保持不变，前端无法读取其明文。</p>}
              {form.credentialAction === "clear" && <p className="text-xs text-destructive">保存后将从安全存储删除该服务的密钥。</p>}
              </>}
            </div>
            <div className="space-y-2"><div className="flex items-center justify-between"><Label htmlFor="provider-models">模型 ID</Label><span className="text-xs text-muted-foreground">每行一个，也可用逗号分隔</span></div><Textarea id="provider-models" value={form.models} onChange={(event) => setForm((current) => ({ ...current, models: event.target.value }))} placeholder={'gpt-5\ngpt-5-mini'} rows={4} /></div>
            <div className="space-y-2"><Label htmlFor="provider-notes">备注</Label><Textarea id="provider-notes" value={form.notes} onChange={(event) => setForm((current) => ({ ...current, notes: event.target.value }))} placeholder="例如：仅用于开发环境" rows={2} /></div>
            <div className="space-y-2"><div><Label>接入应用</Label><p className="mt-1 text-xs text-muted-foreground">只列出当前探测到的应用；应用仍可退出共享并保留自己的配置。</p></div><AppChooser apps={availableApps} value={form.appTypes} onChange={(appTypes) => setForm((current) => ({ ...current, appTypes }))} /></div>
            <label className="flex items-center justify-between rounded-xl border border-border/70 px-3 py-3"><div><p className="text-sm font-medium">启用此服务</p><p className="text-xs text-muted-foreground">停用后不会继续应用到其他应用。</p></div><Switch checked={form.enabled} onCheckedChange={(enabled) => setForm((current) => ({ ...current, enabled }))} /></label>
          </div>
          <DialogFooter><Button variant="outline" onClick={() => setFormOpen(false)}>取消</Button><Button onClick={() => void save()} disabled={busy}>{busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}保存</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={importsOpen} onOpenChange={setImportsOpen}>
        <DialogContent className="max-h-[90vh] max-w-3xl overflow-y-auto">
          <DialogHeader><DialogTitle>导入本机模型服务</DialogTitle><DialogDescription>导入只复制，不修改来源应用。共发现 {candidates.length} 项配置，其中 {importSummary} 项包含可安全迁移的密钥。</DialogDescription></DialogHeader>
          <div className="space-y-3 py-2">
            {importErrors.length > 0 && <div className="rounded-xl border border-amber-500/30 bg-amber-500/5 p-3 text-sm text-amber-800 dark:text-amber-200">部分应用配置无法读取：{importErrors.join("；")}</div>}
            {candidates.length === 0 ? <div className="rounded-xl border border-dashed p-8 text-center text-sm text-muted-foreground">没有找到可导入的配置。你仍可手动添加模型服务。</div> : candidates.map((candidate) => (
              <section key={candidate.sourceRef} className="rounded-xl border border-border/70 p-4">
                <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                  <div className="min-w-0"><div className="flex flex-wrap items-center gap-2"><h4 className="font-medium">{candidate.name}</h4><Badge variant="outline">{appLabel(candidate.sourceApp)}</Badge>{candidate.credentialConfigured ? <Badge className="border-emerald-500/30 bg-emerald-500/10 text-emerald-700 hover:bg-emerald-500/10 dark:text-emerald-300">包含密钥</Badge> : <Badge variant="secondary">无可读取密钥</Badge>}</div><p className="mt-1 break-all text-sm text-muted-foreground">{candidate.baseUrl}</p>{candidate.credentialHint && <p className="mt-1 text-xs text-muted-foreground">密钥：{candidate.credentialHint}</p>}</div>
                  <Button size="sm" onClick={() => void importCandidate(candidate)} disabled={busy || (importApps[candidate.sourceRef]?.length ?? 0) === 0}><Download className="mr-2 h-4 w-4" />复制并保存</Button>
                </div>
                {candidate.models.length > 0 && <div className="mt-3 flex flex-wrap gap-1.5">{candidate.models.map((model) => <Badge key={model} variant="secondary" className="font-normal">{model}</Badge>)}</div>}
                <div className="mt-4 space-y-2"><p className="text-xs font-medium text-muted-foreground">导入后关联到</p><AppChooser apps={availableApps} value={importApps[candidate.sourceRef] ?? []} onChange={(appTypes) => setImportApps((current) => ({ ...current, [candidate.sourceRef]: appTypes }))} /></div>
              </section>
            ))}
          </div>
          <DialogFooter><Button variant="outline" onClick={() => setImportsOpen(false)}>关闭</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={Boolean(preview)} onOpenChange={(open) => { if (!open) { setPreview(null); setPreviewDefinition(null); } }}>
        <DialogContent className="max-w-2xl">
          <DialogHeader><DialogTitle>确认应用“{previewDefinition?.name}”</DialogTitle><DialogDescription>只有确认后才会修改目标配置，并且会先创建恢复快照。</DialogDescription></DialogHeader>
          <div className="space-y-3 py-2">
            {preview?.targets.map((target) => (
              <div key={target.appType} className="flex items-start justify-between gap-3 rounded-xl border border-border/70 p-3">
                <div><p className="text-sm font-medium">{appLabel(target.appType)}</p><p className="mt-1 text-xs text-muted-foreground">{target.message || (target.operation === "create" ? "将创建新的应用配置" : target.operation === "update" ? "将更新现有应用配置" : "当前不支持自动应用")}</p></div>
                {!target.compatible ? <Badge variant="destructive">不兼容</Badge> : target.drifted ? <Badge className="border-orange-500/30 bg-orange-500/10 text-orange-700 hover:bg-orange-500/10 dark:text-orange-300">外部配置已变化</Badge> : <Badge className="border-emerald-500/30 bg-emerald-500/10 text-emerald-700 hover:bg-emerald-500/10 dark:text-emerald-300">可以应用</Badge>}
              </div>
            ))}
            {previewBlocked && <div className="flex gap-2 rounded-xl border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"><AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" /><p>存在不兼容目标或配置已在预览后发生变化。请处理后重新生成预览，当前不会写入。</p></div>}
          </div>
          <DialogFooter><Button variant="outline" onClick={() => { setPreview(null); setPreviewDefinition(null); }}>取消</Button><Button onClick={() => void confirmApply()} disabled={busy || previewBlocked}>{busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}确认并应用</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={Boolean(transactionResult)} onOpenChange={(open) => { if (!open) setTransactionResult(null); }}>
        <DialogContent className="max-w-2xl">
          <DialogHeader><DialogTitle>应用结果</DialogTitle><DialogDescription>每个应用都在写入后重新读取并校验；失败项不会被当作成功。</DialogDescription></DialogHeader>
          <div className="space-y-3 py-2">
            <div className="flex items-center gap-2 rounded-xl bg-muted/40 p-3">{transactionResult?.status === "applied" || transactionResult?.status === "restored" ? <CheckCircle2 className="h-5 w-5 text-emerald-600" /> : <AlertTriangle className="h-5 w-5 text-destructive" />}<div><p className="text-sm font-medium">状态：{transactionResult?.status}</p><p className="text-xs text-muted-foreground">事务 {transactionResult?.id} · {formatTime(transactionResult?.completedAt ?? transactionResult?.createdAt)}</p></div></div>
            {transactionResult?.targets.map((target) => <div key={target.appType} className="flex items-start justify-between gap-3 rounded-xl border border-border/70 p-3"><div><p className="text-sm font-medium">{appLabel(target.appType)}</p>{target.message && <p className="mt-1 text-xs text-muted-foreground">{target.message}</p>}</div><Badge variant={target.status === "applied" || target.status === "restored" || target.status === "rolled_back" ? "secondary" : "destructive"}>{target.status}</Badge></div>)}
            {transactionResult?.status === "recoveryRequired" && <div className="flex gap-2 rounded-xl border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"><AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />自动恢复未完全成功，请从应用历史再次恢复；仍失败时请保留日志并手动恢复对应配置。</div>}
          </div>
          <DialogFooter>{transactionResult && transactionResult.status !== "applied" && transactionResult.status !== "restored" && <Button variant="outline" onClick={() => void restore(transactionResult)} disabled={busy}><RotateCcw className="mr-2 h-4 w-4" />尝试恢复</Button>}<Button onClick={() => setTransactionResult(null)}>完成</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={historyOpen} onOpenChange={setHistoryOpen}>
        <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
          <DialogHeader><DialogTitle>应用历史</DialogTitle><DialogDescription>查看逐应用结果，并在需要时恢复到应用前的快照。</DialogDescription></DialogHeader>
          <div className="space-y-3 py-2">{transactions.length === 0 ? <p className="rounded-xl border border-dashed p-8 text-center text-sm text-muted-foreground">暂无应用记录。</p> : transactions.map((transaction) => { const definition = definitions.find((item) => item.id === transaction.providerId); return <div key={transaction.id} className="rounded-xl border border-border/70 p-3"><div className="flex items-start justify-between gap-3"><div><p className="text-sm font-medium">{definition?.name ?? "已删除的模型服务"}</p><p className="mt-1 text-xs text-muted-foreground">{formatTime(transaction.completedAt ?? transaction.createdAt)} · {transaction.targets.map((target) => appLabel(target.appType)).join("、")}</p></div><Badge variant={transaction.status === "applied" || transaction.status === "restored" ? "secondary" : "destructive"}>{transaction.status}</Badge></div><div className="mt-3 flex justify-end"><Button size="sm" variant="outline" onClick={() => void restore(transaction)} disabled={busy || transaction.status === "restored"}><RotateCcw className="mr-2 h-4 w-4" />恢复此记录</Button></div></div>; })}</div>
          <DialogFooter><Button variant="outline" onClick={() => setHistoryOpen(false)}>关闭</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={Boolean(deleteTarget)} onOpenChange={(open) => { if (!open) setDeleteTarget(null); }}>
        <DialogContent>
          <DialogHeader><DialogTitle>删除“{deleteTarget?.name}”？</DialogTitle><DialogDescription>这会删除共享定义和安全存储中的对应密钥，但不会静默删除各应用现有的独立配置。</DialogDescription></DialogHeader>
          <div className="flex gap-2 rounded-xl border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"><Unlink className="mt-0.5 h-4 w-4 shrink-0" />如果该服务已经应用到某个应用，请先从应用历史恢复或在该应用中改用其他配置。</div>
          <DialogFooter><Button variant="outline" onClick={() => setDeleteTarget(null)}>取消</Button><Button variant="destructive" onClick={() => void remove()} disabled={busy}>{busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}确认删除</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
