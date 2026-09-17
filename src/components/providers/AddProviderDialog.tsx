import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Plus } from "lucide-react";
import { toast } from "sonner";
import { useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import type { Provider, CustomEndpoint } from "@/types";
import type { AppId } from "@/lib/api";
import { providerCenterApi } from "@/lib/api";
import type {
  ManagedProviderDraftInput,
  ProviderApplyPreview,
  ProviderCenterApp,
} from "@/lib/api/providerCenter";
import { UniversalProviderTargetDialog } from "@/components/providers/UniversalProviderTargetDialog";
import {
  PROVIDER_CENTER_APPS,
  providerCenterAppLabel,
} from "@/components/providers/providerCenterApps";
import {
  ProviderForm,
  type ProviderFormValues,
} from "@/components/providers/forms/ProviderForm";
import { AuthSettingsPanel } from "@/components/providers/AuthSettingsPanel";
import { providerPresets } from "@/config/claudeProviderPresets";
import { codexProviderPresets } from "@/config/codexProviderPresets";
import { geminiProviderPresets } from "@/config/geminiProviderPresets";
import { claudeDesktopProviderPresets } from "@/config/claudeDesktopProviderPresets";
import { extractCodexBaseUrl } from "@/utils/providerConfigUtils";
import { extractGrokBuildBaseUrl } from "@/utils/grokBuildConfig";
import { extractErrorMessage } from "@/utils/errorUtils";
import { GROKBUILD_OFFICIAL_PROVIDER_ID } from "@/utils/providerCapabilities";
import type { OpenClawSuggestedDefaults } from "@/config/openclawProviderPresets";
import type { ManagedAuthProvider } from "@/lib/api";

interface AddProviderDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  appId: AppId;
  onSubmit: (
    provider: Omit<Provider, "id"> & {
      providerKey?: string;
      suggestedDefaults?: OpenClawSuggestedDefaults;
      ensureClaudeDesktopOfficialSeed?: boolean;
      ensureGrokBuildOfficialSeed?: boolean;
    },
  ) => Promise<void> | void;
}

export function AddProviderDialog({
  open,
  onOpenChange,
  appId,
  onSubmit,
}: AddProviderDialogProps) {
  const { t } = useTranslation();
  // All agents can potentially save as universal; only Official/OAuth
  // presets are excluded at submit time based on the form values.
  const canSaveAsUniversal = true;
  const [saveScope, setSaveScope] = useState<"universal" | "agentOnly">(
    "agentOnly",
  );
  const [isFormSubmitting, setIsFormSubmitting] = useState(false);
  const [authSettingsTarget, setAuthSettingsTarget] =
    useState<ManagedAuthProvider | null>(null);
  const [managedDraftDiscovery, setManagedDraftDiscovery] = useState<{
    preview: ProviderApplyPreview;
    input: ManagedProviderDraftInput;
  } | null>(null);
  const [selectedTargetApps, setSelectedTargetApps] = useState<
    ProviderCenterApp[]
  >([]);
  const [managedDraftPreview, setManagedDraftPreview] = useState<{
    preview: ProviderApplyPreview;
    input: ManagedProviderDraftInput;
  } | null>(null);
  const [managedDraftPreviewing, setManagedDraftPreviewing] = useState(false);
  const [managedDraftApplying, setManagedDraftApplying] = useState(false);
  const queryClient = useQueryClient();

  useEffect(() => {
    setAuthSettingsTarget(null);
    setSaveScope("agentOnly");
    setManagedDraftDiscovery(null);
    setManagedDraftPreview(null);
    setSelectedTargetApps([]);
  }, [appId, open, canSaveAsUniversal]);

  const closeDialog = useCallback(() => {
    setAuthSettingsTarget(null);
    setManagedDraftDiscovery(null);
    setManagedDraftPreview(null);
    setSelectedTargetApps([]);
    onOpenChange(false);
  }, [onOpenChange]);

  const handlePanelClose = useCallback(() => {
    if (authSettingsTarget) {
      setAuthSettingsTarget(null);
      return;
    }
    closeDialog();
  }, [authSettingsTarget, closeDialog]);
  const formReadyToken = useMemo(
    () => Symbol("provider-form-ready"),
    [appId, open],
  );
  const currentFormReadyToken = useRef(formReadyToken);
  currentFormReadyToken.current = formReadyToken;
  const [formReadyState, setFormReadyState] = useState({
    token: formReadyToken,
    ready: appId !== "pi",
  });
  const isFormReady =
    formReadyState.token === formReadyToken
      ? formReadyState.ready
      : appId !== "pi";
  const handleSubmitReadyChange = useCallback(
    (ready: boolean) => {
      if (currentFormReadyToken.current === formReadyToken) {
        setFormReadyState({ token: formReadyToken, ready });
      }
    },
    [formReadyToken],
  );

  const handleSubmit = useCallback(
    async (values: ProviderFormValues) => {
      const parsedConfig = JSON.parse(values.settingsConfig) as Record<
        string,
        unknown
      >;

      // 构造基础提交数据
      const providerData: Omit<Provider, "id"> & {
        providerKey?: string;
        suggestedDefaults?: OpenClawSuggestedDefaults;
        ensureClaudeDesktopOfficialSeed?: boolean;
        ensureGrokBuildOfficialSeed?: boolean;
      } = {
        name: values.name.trim(),
        notes: values.notes?.trim() || undefined,
        websiteUrl: values.websiteUrl?.trim() || undefined,
        settingsConfig: parsedConfig,
        icon: values.icon?.trim() || undefined,
        iconColor: values.iconColor?.trim() || undefined,
        ...(values.presetCategory ? { category: values.presetCategory } : {}),
        ...(values.meta ? { meta: values.meta } : {}),
      };
      if (appId === "claude-desktop" && values.presetId) {
        const presetIndex = parseInt(
          values.presetId.replace("claude-desktop-", ""),
        );
        const preset = claudeDesktopProviderPresets[presetIndex];
        providerData.ensureClaudeDesktopOfficialSeed =
          values.presetCategory === "official" &&
          preset?.category === "official";
      }

      if (appId === "grokbuild" && values.presetId) {
        providerData.ensureGrokBuildOfficialSeed =
          values.presetCategory === "official" &&
          values.presetId === GROKBUILD_OFFICIAL_PROVIDER_ID;
      }

      // Apps whose native catalog has a stable provider key use it as the
      // managed provider identity.
      if (
        (appId === "opencode" ||
          appId === "openclaw" ||
          appId === "hermes" ||
          appId === "pi") &&
        values.providerKey
      ) {
        providerData.providerKey = values.providerKey;
      }
      const hasCustomEndpoints =
        providerData.meta?.custom_endpoints &&
        Object.keys(providerData.meta.custom_endpoints).length > 0;

      if (!hasCustomEndpoints && values.presetCategory !== "omo") {
        const urlSet = new Set<string>();

        const addUrl = (rawUrl?: string) => {
          const url = (rawUrl || "").trim().replace(/\/+$/, "");
          if (url && url.startsWith("http")) {
            urlSet.add(url);
          }
        };

        if (values.presetId) {
          if (appId === "claude") {
            const presets = providerPresets;
            const presetIndex = parseInt(
              values.presetId.replace("claude-", ""),
            );
            if (
              !isNaN(presetIndex) &&
              presetIndex >= 0 &&
              presetIndex < presets.length
            ) {
              const preset = presets[presetIndex];
              if (preset?.endpointCandidates) {
                preset.endpointCandidates.forEach(addUrl);
              }
            }
          } else if (appId === "codex") {
            const presets = codexProviderPresets;
            const presetIndex = parseInt(values.presetId.replace("codex-", ""));
            if (
              !isNaN(presetIndex) &&
              presetIndex >= 0 &&
              presetIndex < presets.length
            ) {
              const preset = presets[presetIndex];
              if (Array.isArray(preset.endpointCandidates)) {
                preset.endpointCandidates.forEach(addUrl);
              }
            }
          } else if (appId === "gemini") {
            const presets = geminiProviderPresets;
            const presetIndex = parseInt(
              values.presetId.replace("gemini-", ""),
            );
            if (
              !isNaN(presetIndex) &&
              presetIndex >= 0 &&
              presetIndex < presets.length
            ) {
              const preset = presets[presetIndex];
              if (Array.isArray(preset.endpointCandidates)) {
                preset.endpointCandidates.forEach(addUrl);
              }
            }
          } else if (appId === "claude-desktop") {
            const presets = claudeDesktopProviderPresets;
            const presetIndex = parseInt(
              values.presetId.replace("claude-desktop-", ""),
            );
            if (
              !isNaN(presetIndex) &&
              presetIndex >= 0 &&
              presetIndex < presets.length
            ) {
              const preset = presets[presetIndex];
              if (Array.isArray(preset.endpointCandidates)) {
                preset.endpointCandidates.forEach(addUrl);
              }
              addUrl(preset.baseUrl);
            }
          }
        }

        if (appId === "claude") {
          const env = parsedConfig.env as Record<string, any> | undefined;
          if (env?.ANTHROPIC_BASE_URL) {
            addUrl(env.ANTHROPIC_BASE_URL);
          }
        } else if (appId === "claude-desktop") {
          const env = parsedConfig.env as Record<string, any> | undefined;
          if (env?.ANTHROPIC_BASE_URL) {
            addUrl(env.ANTHROPIC_BASE_URL);
          }
        } else if (appId === "codex") {
          const config = parsedConfig.config as string | undefined;
          if (config) {
            const extractedBaseUrl = extractCodexBaseUrl(config);
            if (extractedBaseUrl) {
              addUrl(extractedBaseUrl);
            }
          }
        } else if (appId === "gemini") {
          const env = parsedConfig.env as Record<string, any> | undefined;
          if (env?.GOOGLE_GEMINI_BASE_URL) {
            addUrl(env.GOOGLE_GEMINI_BASE_URL);
          }
        } else if (appId === "grokbuild") {
          const config = parsedConfig.config as string | undefined;
          if (config) {
            addUrl(extractGrokBuildBaseUrl(config));
          }
        } else if (appId === "opencode") {
          const options = parsedConfig.options as
            | Record<string, any>
            | undefined;
          if (options?.baseURL) {
            addUrl(options.baseURL);
          }
        } else if (appId === "openclaw") {
          // OpenClaw uses baseUrl directly
          if (parsedConfig.baseUrl) {
            addUrl(parsedConfig.baseUrl as string);
          }
        } else if (appId === "hermes") {
          if (parsedConfig.base_url) {
            addUrl(parsedConfig.base_url as string);
          }
        }

        const urls = Array.from(urlSet);
        if (urls.length > 0) {
          const now = Date.now();
          const customEndpoints: Record<string, CustomEndpoint> = {};
          urls.forEach((url) => {
            customEndpoints[url] = {
              url,
              addedAt: now,
              lastUsed: undefined,
            };
          });

          providerData.meta = {
            ...(providerData.meta ?? {}),
            custom_endpoints: customEndpoints,
          };
        }
      }

      // OpenClaw: pass suggestedDefaults for model registration
      if (appId === "openclaw" && values.suggestedDefaults) {
        providerData.suggestedDefaults = values.suggestedDefaults;
      }

      if (saveScope === "universal") {
        const definitionId = crypto.randomUUID();
        const provider: Provider = {
          ...providerData,
          id: crypto.randomUUID(),
        };
        try {
          const input: ManagedProviderDraftInput = {
            appType: appId,
            provider,
            definitionId,
            targetAppTypes: PROVIDER_CENTER_APPS,
          };
          setManagedDraftPreviewing(true);
          const preview = await providerCenterApi.previewManagedDraft(input);
          setManagedDraftDiscovery({ preview, input });
          setSelectedTargetApps(
            PROVIDER_CENTER_APPS.filter((candidate) =>
              preview.targets.some(
                (target) =>
                  target.appType === candidate && target.compatible,
              ),
            ),
          );
          return;
        } catch (error) {
          toast.error(extractErrorMessage(error));
          return;
        } finally {
          setManagedDraftPreviewing(false);
        }
      }

      await onSubmit(providerData);
      closeDialog();
    },
    [appId, onSubmit, closeDialog, saveScope],
  );

  const handlePreviewSelectedTargets = useCallback(async () => {
    if (!managedDraftDiscovery || selectedTargetApps.length === 0) return;
    setManagedDraftPreviewing(true);
    try {
      const input: ManagedProviderDraftInput = {
        ...managedDraftDiscovery.input,
        targetAppTypes: selectedTargetApps,
      };
      const preview = await providerCenterApi.previewManagedDraft(input);
      const incompatible = preview.targets.find((target) => !target.compatible);
      if (incompatible) {
        toast.error(
          `${providerCenterAppLabel(incompatible.appType)} 不兼容${incompatible.message ? `：${incompatible.message}` : ""}`,
        );
        return;
      }
      setManagedDraftDiscovery(null);
      setManagedDraftPreview({ preview, input });
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setManagedDraftPreviewing(false);
    }
  }, [managedDraftDiscovery, selectedTargetApps]);

  const handleCancelTargetSelection = useCallback(() => {
    setManagedDraftDiscovery(null);
    setSelectedTargetApps([]);
  }, []);

  const handleConfirmManagedDraft = useCallback(async () => {
    if (!managedDraftPreview) return;
    const { preview, input } = managedDraftPreview;
    setManagedDraftApplying(true);
    try {
      await providerCenterApi.confirmManagedDraft(input, preview.token);
      await queryClient.invalidateQueries({
        queryKey: ["providers", appId],
      });
      await queryClient.invalidateQueries({
        queryKey: ["provider-center"],
      });
      toast.success("通用 Provider 已添加");
      setManagedDraftPreview(null);
      closeDialog();
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setManagedDraftApplying(false);
    }
  }, [managedDraftPreview, queryClient, appId, closeDialog]);

  const handleCancelManagedDraft = useCallback(() => {
    setManagedDraftPreview(null);
  }, []);

  const footer = (
    <>
      <span className="mr-auto min-w-0 text-xs text-muted-foreground truncate">
        {t("provider.addFooterHint")}
      </span>
      <Button
        variant="outline"
        onClick={closeDialog}
        className="border-border/20 hover:bg-accent hover:text-accent-foreground"
      >
        {t("common.cancel")}
      </Button>
      <Button
        type="submit"
        form="provider-form"
        disabled={
          isFormSubmitting ||
          managedDraftPreviewing ||
          !isFormReady
        }
        className="bg-primary text-primary-foreground hover:bg-primary/90"
      >
        {isFormSubmitting || managedDraftPreviewing ? (
          <Loader2 className="mr-2 h-4 w-4 animate-spin" />
        ) : (
          <Plus className="mr-2 h-4 w-4" />
        )}
        {saveScope === "universal"
          ? t("universalProvider.add")
          : t("common.add")}
      </Button>
    </>
  );

  return (
    <FullScreenPanel
      isOpen={open}
      title={t("provider.addNewProvider")}
      onClose={handlePanelClose}
      footer={footer}
      contentClassName={appId === "pi" ? "pt-3 pb-0" : "pt-3"}
    >
      {canSaveAsUniversal && (
        <div className="flex items-center gap-2 mb-4 px-1">
          <span className="text-xs text-muted-foreground shrink-0">
            保存范围
          </span>
          <Button
            variant={saveScope === "universal" ? "default" : "outline"}
            size="sm"
            onClick={() => setSaveScope("universal")}
            className="h-7 text-xs"
          >
            通用 Provider
          </Button>
          <Button
            variant={saveScope === "agentOnly" ? "default" : "outline"}
            size="sm"
            onClick={() => setSaveScope("agentOnly")}
            className="h-7 text-xs"
          >
            仅当前 Agent
          </Button>
        </div>
      )}

      <ProviderForm
        appId={appId}
        submitLabel={t("common.add")}
        onSubmit={handleSubmit}
        onCancel={closeDialog}
        onManageAuthAccounts={setAuthSettingsTarget}
        onSubmittingChange={setIsFormSubmitting}
        onSubmitReadyChange={handleSubmitReadyChange}
        showButtons={false}
      />

      <AuthSettingsPanel
        target={authSettingsTarget}
        onClose={() => setAuthSettingsTarget(null)}
      />

      <UniversalProviderTargetDialog
        open={Boolean(managedDraftDiscovery)}
        targets={managedDraftDiscovery?.preview.targets ?? []}
        selectedAppTypes={selectedTargetApps}
        pending={managedDraftPreviewing}
        onSelectedAppTypesChange={setSelectedTargetApps}
        onConfirm={() => void handlePreviewSelectedTargets()}
        onCancel={handleCancelTargetSelection}
      />

      <ConfirmDialog
        isOpen={Boolean(managedDraftPreview)}
        title="添加通用 Provider"
        message={
          managedDraftPreview
            ? managedDraftPreview.preview.targets
                .map(
                  (target) =>
                    `${providerCenterAppLabel(target.appType)}：${target.operation === "create" ? "创建" : "更新"} · ${target.connectionMode === "proxy" ? "需要路由" : "直接兼容"}${target.drifted ? "（检测到漂移）" : ""}${target.message ? ` — ${target.message}` : ""}`,
                )
                .join("\n")
            : ""
        }
        confirmText="确认添加"
        cancelText={t("common.cancel")}
        variant="info"
        zIndex="top"
        pending={managedDraftApplying}
        onConfirm={() => void handleConfirmManagedDraft()}
        onCancel={handleCancelManagedDraft}
      />
    </FullScreenPanel>
  );
}
