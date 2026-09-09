import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import {
  Check,
  ChevronDown,
  CircleAlert,
  KeyRound,
  Loader2,
  Rocket,
  Server,
  Sparkles,
  Terminal,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { CodexIcon } from "@/components/BrandIcons";
import { useSettingsQuery } from "@/lib/query";
import { settingsApi } from "@/lib/api";
import { providerCenterApi } from "@/lib/api/providerCenter";
import { extractErrorMessage } from "@/utils/errorUtils";
import { cn } from "@/lib/utils";

type CodexRuntime = {
  version: string | null;
  installed_but_broken: boolean;
};

type ApiFormat = "openai_chat" | "openai_responses" | "anthropic";

interface QuickStartOnboardingProps {
  onComplete?: () => void;
}

/**
 * A deliberately small first-run path. It creates the same shared Provider
 * definition used by Provider Center, then explicitly applies its Codex
 * binding. There is no onboarding-only configuration format.
 */
export function QuickStartOnboarding({
  onComplete,
}: QuickStartOnboardingProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: settings } = useSettingsQuery();
  const [dismissed, setDismissed] = useState(false);
  const [runtime, setRuntime] = useState<CodexRuntime | null>(null);
  const [checkingRuntime, setCheckingRuntime] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [saving, setSaving] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [apiFormat, setApiFormat] = useState<ApiFormat>("openai_chat");
  const [model, setModel] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const isOpen =
    settings != null &&
    settings.quickStartOnboardingCompleted !== true &&
    !dismissed;

  const refreshRuntime = useCallback(async () => {
    setCheckingRuntime(true);
    try {
      const [next] = await settingsApi.getToolVersions(["codex"]);
      setRuntime(next ?? null);
    } catch (error) {
      toast.error(t("quickStart.runtimeCheckFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    } finally {
      setCheckingRuntime(false);
    }
  }, [t]);

  useEffect(() => {
    if (isOpen) void refreshRuntime();
  }, [isOpen, refreshRuntime]);

  const finish = async () => {
    if (!settings) return;
    const { webdavSync: _, ...rest } = settings;
    await settingsApi.save({
      ...rest,
      quickStartOnboardingCompleted: true,
      // This quick-start replaces the older one-button welcome notice for a
      // fresh install, so beginners are never shown two consecutive modals.
      firstRunNoticeConfirmed: true,
    });
    setDismissed(true);
    await queryClient.invalidateQueries({ queryKey: ["settings"] });
    onComplete?.();
  };

  const installCodex = async () => {
    setInstalling(true);
    try {
      await settingsApi.runToolLifecycleAction(["codex"], "install");
      await refreshRuntime();
      toast.success(t("quickStart.runtimeInstalled"));
    } catch (error) {
      toast.error(t("quickStart.runtimeInstallFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    } finally {
      setInstalling(false);
    }
  };

  const saveProvider = async () => {
    const cleanName = name.trim();
    const cleanUrl = baseUrl.trim().replace(/\/+$/, "");
    const cleanKey = apiKey.trim();
    setFormError(null);

    if (!cleanName || !cleanUrl || !cleanKey) {
      setFormError(t("quickStart.requiredFields"));
      return;
    }
    try {
      const url = new URL(cleanUrl);
      if (url.protocol !== "http:" && url.protocol !== "https:") {
        throw new Error("Unsupported URL scheme");
      }
    } catch {
      setFormError(t("quickStart.urlInvalid"));
      return;
    }

    setSaving(true);
    try {
      const protocol = apiFormat === "openai_chat"
        ? "openai-chat"
        : apiFormat === "openai_responses"
          ? "openai-responses"
          : "anthropic";
      const definition = await providerCenterApi.save({
        name: cleanName,
        baseUrl: cleanUrl,
        protocol,
        models: model.trim() ? [model.trim()] : [],
        notes: "由首次启动引导创建",
        enabled: true,
        credentialAction: "replace",
        apiKey: cleanKey,
        appTypes: ["codex"],
      });
      const preview = await providerCenterApi.previewApply(definition.id, ["codex"]);
      const result = await providerCenterApi.applyTransaction(
        definition.id,
        ["codex"],
        preview.token,
      );
      if (result.status !== "applied") {
        throw new Error(`配置未能完整应用（${result.status}）`);
      }
      await queryClient.invalidateQueries({ queryKey: ["providers", "codex"] });
      await finish();
      toast.success(t("quickStart.providerSaved"));
    } catch (error) {
      toast.error(t("quickStart.providerSaveFailed"), {
        description: extractErrorMessage(error) || undefined,
      });
    } finally {
      setSaving(false);
    }
  };

  const cliReady = Boolean(runtime?.version && !runtime?.installed_but_broken);

  return (
    <div
      aria-hidden={!isOpen}
      className={cn(
        "fixed inset-0 z-[120] overflow-y-auto bg-[radial-gradient(circle_at_18%_10%,rgba(124,58,237,0.15),transparent_30%),radial-gradient(circle_at_90%_85%,rgba(14,165,233,0.10),transparent_28%),hsl(var(--background))]",
        isOpen ? "block" : "hidden",
      )}
    >
      <div className="mx-auto flex min-h-full max-w-5xl items-center px-5 py-10 sm:px-8">
        <div className="grid w-full overflow-hidden rounded-[28px] border border-border/80 bg-background/90 shadow-2xl shadow-violet-950/10 backdrop-blur md:grid-cols-[0.86fr_1.14fr]">
          <section className="relative overflow-hidden border-b border-border/70 bg-muted/30 p-7 md:border-b-0 md:border-r md:p-10">
            <div className="absolute -right-20 -top-20 h-56 w-56 rounded-full bg-violet-500/10 blur-3xl" />
            <div className="relative flex h-full flex-col">
              <span className="mb-6 inline-flex h-12 w-12 items-center justify-center rounded-2xl border border-violet-500/20 bg-violet-500/10">
                <CodexIcon size={24} className="dark:invert-0" />
              </span>
              <p className="text-xs font-semibold uppercase tracking-[0.18em] text-violet-600 dark:text-violet-300">
                {t("quickStart.eyebrow")}
              </p>
              <h1 className="mt-3 max-w-sm text-3xl font-semibold tracking-tight">
                {t("quickStart.title")}
              </h1>
              <p className="mt-4 max-w-sm text-sm leading-6 text-muted-foreground">
                {t("quickStart.intro")}
              </p>

              <div className="mt-8 space-y-3">
                {["stepAssistant", "stepProvider", "stepHelp"].map((key, index) => (
                  <div key={key} className="flex items-start gap-3 text-sm">
                    <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-violet-500/10 text-xs font-semibold text-violet-700 dark:text-violet-200">
                      {index + 1}
                    </span>
                    <span className="pt-0.5 text-muted-foreground">{t(`quickStart.${key}`)}</span>
                  </div>
                ))}
              </div>

              <button
                type="button"
                onClick={() => void finish()}
                className="mt-10 w-fit text-sm text-muted-foreground underline-offset-4 transition hover:text-foreground hover:underline"
              >
                {t("quickStart.skip")}
              </button>
            </div>
          </section>

          <section className="p-7 md:p-10">
            <div className="mb-7 flex items-start gap-3">
              <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-violet-500/10 text-violet-700 dark:text-violet-200">
                <Sparkles className="h-5 w-5" />
              </span>
              <div>
                <h2 className="font-semibold">{t("quickStart.setupTitle")}</h2>
                <p className="mt-1 text-sm text-muted-foreground">{t("quickStart.setupHint")}</p>
              </div>
            </div>

            <div className="mb-6 rounded-2xl border border-border bg-card p-4">
              <div className="flex items-center gap-3">
                <span className={cn("flex h-9 w-9 items-center justify-center rounded-xl", cliReady ? "bg-emerald-500/10 text-emerald-600" : "bg-amber-500/10 text-amber-600")}>
                  {checkingRuntime || installing ? <Loader2 className="h-4 w-4 animate-spin" /> : cliReady ? <Check className="h-4 w-4" /> : <Terminal className="h-4 w-4" />}
                </span>
                <div className="min-w-0 flex-1">
                  <p className="text-sm font-medium">{t("quickStart.codexCli")}</p>
                  <p className="text-xs text-muted-foreground">
                    {cliReady ? t("quickStart.cliReady", { version: runtime?.version }) : t("quickStart.cliHint")}
                  </p>
                </div>
                {!cliReady && (
                  <Button size="sm" onClick={() => void installCodex()} disabled={checkingRuntime || installing}>
                    {installing ? <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" /> : <Rocket className="mr-1.5 h-3.5 w-3.5" />}
                    {t("quickStart.installCli")}
                  </Button>
                )}
              </div>
            </div>

            <div className="space-y-4">
              <div className="space-y-2">
                <Label htmlFor="quick-provider-name">{t("quickStart.providerName")}</Label>
                <Input id="quick-provider-name" value={name} onChange={(event) => setName(event.target.value)} placeholder={t("quickStart.providerNamePlaceholder")} />
              </div>
              <div className="space-y-2">
                <Label htmlFor="quick-provider-url">{t("quickStart.baseUrl")}</Label>
                <Input id="quick-provider-url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" inputMode="url" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="quick-provider-key">{t("quickStart.apiKey")}</Label>
                <div className="relative">
                  <KeyRound className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                  <Input id="quick-provider-key" type="password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} className="pl-9" placeholder={t("quickStart.apiKeyPlaceholder")} autoComplete="off" />
                </div>
              </div>
            </div>

            <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen} className="mt-5 rounded-xl border border-border/70">
              <CollapsibleTrigger className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left text-sm font-medium hover:bg-muted/50">
                <span className="flex items-center gap-2"><Server className="h-4 w-4 text-muted-foreground" />{t("quickStart.advanced")}</span>
                <ChevronDown className={cn("h-4 w-4 text-muted-foreground transition-transform", advancedOpen && "rotate-180")} />
              </CollapsibleTrigger>
              <CollapsibleContent className="border-t border-border/70 px-4 pb-4 pt-4">
                <p className="mb-4 text-xs leading-5 text-muted-foreground">{t("quickStart.advancedHint")}</p>
                <div className="grid gap-4 sm:grid-cols-2">
                  <div className="space-y-2">
                    <Label htmlFor="quick-protocol">{t("quickStart.protocol")}</Label>
                    <select id="quick-protocol" value={apiFormat} onChange={(event) => setApiFormat(event.target.value as ApiFormat)} className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-sm shadow-sm outline-none focus-visible:ring-1 focus-visible:ring-ring">
                      <option value="openai_chat">OpenAI Chat Completions</option>
                      <option value="openai_responses">OpenAI Responses</option>
                      <option value="anthropic">Anthropic Messages</option>
                    </select>
                  </div>
                  <div className="space-y-2">
                    <Label htmlFor="quick-model">{t("quickStart.model")}</Label>
                    <Input id="quick-model" value={model} onChange={(event) => setModel(event.target.value)} placeholder={t("quickStart.modelPlaceholder")} />
                  </div>
                </div>
              </CollapsibleContent>
            </Collapsible>

            {formError && <p className="mt-4 flex items-center gap-2 text-sm text-destructive"><CircleAlert className="h-4 w-4" />{formError}</p>}

            <div className="mt-6 flex items-center justify-end gap-3">
              <Button variant="ghost" onClick={() => void finish()} disabled={saving}>{t("quickStart.skip")}</Button>
              <Button onClick={() => void saveProvider()} disabled={saving}>
                {saving ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : <Check className="mr-2 h-4 w-4" />}
                {t("quickStart.saveAndStart")}
              </Button>
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
