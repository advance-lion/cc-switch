import { useEffect, useMemo, useState } from "react";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Form } from "@/components/ui/form";
import { ImeSafeInput } from "@/components/ui/ime-safe-input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import type { ProviderFormData } from "@/lib/schemas/provider";
import { BasicFormFields } from "./BasicFormFields";
import type { ProviderFormProps } from "./ProviderForm";

export const DSH_DEFAULT_CONFIG = JSON.stringify(
  {
    schemaVersion: 1,
    route: "",
    displayName: "",
    api: "openai-completions",
    baseURL: "",
    apiKey: "",
    models: [],
  },
  null,
  2,
);

const DSH_PROTOCOLS = [
  { value: "openai-completions", label: "OpenAI Chat Completions" },
  { value: "openai-responses", label: "OpenAI Responses" },
  { value: "anthropic-messages", label: "Anthropic Messages" },
  { value: "ollama", label: "Ollama" },
] as const;

const normalizeRoute = (value: string) =>
  value.toLowerCase().replace(/[^a-z0-9-]/g, "");

const asRecord = (value: unknown): Record<string, unknown> =>
  value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

export function DshProviderForm({
  providerId,
  submitLabel,
  onSubmit,
  onCancel,
  onSubmittingChange,
  onSubmitReadyChange,
  initialData,
  showButtons = true,
}: ProviderFormProps) {
  const { t } = useTranslation();
  const initialConfig = useMemo(
    () => asRecord(initialData?.settingsConfig),
    [initialData?.settingsConfig],
  );
  const initialModels = Array.isArray(initialConfig.models)
    ? initialConfig.models
        .map((model) =>
          typeof model === "string"
            ? model
            : typeof model === "object" && model !== null
              ? String((model as Record<string, unknown>).id ?? "")
              : "",
        )
        .filter(Boolean)
        .join("\n")
    : "";
  const [route, setRoute] = useState(
    providerId ??
      (typeof initialConfig.route === "string" ? initialConfig.route : ""),
  );
  const [api, setApi] = useState(
    typeof initialConfig.api === "string"
      ? initialConfig.api
      : "openai-completions",
  );
  const [baseURL, setBaseURL] = useState(
    typeof initialConfig.baseURL === "string" ? initialConfig.baseURL : "",
  );
  const [apiKey, setApiKey] = useState(
    typeof initialConfig.apiKey === "string" ? initialConfig.apiKey : "",
  );
  const [models, setModels] = useState(initialModels);
  const form = useForm<ProviderFormData>({
    defaultValues: {
      name: initialData?.name ?? "",
      websiteUrl: initialData?.websiteUrl ?? "",
      notes: initialData?.notes ?? "",
      settingsConfig: DSH_DEFAULT_CONFIG,
      icon: initialData?.icon ?? "",
      iconColor: initialData?.iconColor ?? "",
    },
  });

  useEffect(() => {
    onSubmitReadyChange?.(true);
  }, [onSubmitReadyChange]);

  useEffect(() => {
    onSubmittingChange?.(form.formState.isSubmitting);
  }, [form.formState.isSubmitting, onSubmittingChange]);

  const handleSubmit = form.handleSubmit(async (values) => {
    if (!route.trim()) {
      toast.error("请填写 DSH Route");
      return;
    }
    if (!values.name.trim()) {
      toast.error(t("providerForm.fillSupplierName"));
      return;
    }

    const modelEntries = models
      .split(/[\n,]/)
      .map((id) => id.trim())
      .filter(Boolean)
      .map((id) => ({ id, name: id }));
    await onSubmit({
      ...values,
      providerKey: route.trim(),
      settingsConfig: JSON.stringify({
        schemaVersion: 1,
        route: route.trim(),
        displayName: values.name.trim(),
        api,
        baseURL: baseURL.trim().replace(/\/+$/, ""),
        apiKey,
        models: modelEntries,
      }),
    });
  });

  return (
    <Form {...form}>
      <form
        id="provider-form"
        onSubmit={(event) => void handleSubmit(event)}
        className="space-y-6 glass rounded-xl p-6 border border-white/10"
      >
        <BasicFormFields
          form={form}
          beforeNameSlot={
            <div className="space-y-2">
              <Label htmlFor="dsh-route">DSH Route</Label>
              <ImeSafeInput
                id="dsh-route"
                value={route}
                onValueChange={(value) => setRoute(normalizeRoute(value))}
                placeholder="my-provider"
                disabled={Boolean(providerId)}
              />
              <p className="text-xs text-muted-foreground">
                DSH 中的稳定供应商标识；添加到 live 配置后不可修改。
              </p>
            </div>
          }
        />

        <div className="space-y-2">
          <Label htmlFor="dsh-api">API 协议</Label>
          <select
            id="dsh-api"
            value={api}
            onChange={(event) => setApi(event.target.value)}
            className="flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
          >
            {DSH_PROTOCOLS.map((protocol) => (
              <option key={protocol.value} value={protocol.value}>
                {protocol.label}
              </option>
            ))}
          </select>
        </div>

        <div className="space-y-2">
          <Label htmlFor="dsh-base-url">API 端点</Label>
          <ImeSafeInput
            id="dsh-base-url"
            value={baseURL}
            onValueChange={setBaseURL}
            placeholder="https://api.example.com/v1"
          />
        </div>

        {api !== "ollama" && (
          <div className="space-y-2">
            <Label htmlFor="dsh-api-key">API Key</Label>
            <ImeSafeInput
              id="dsh-api-key"
              type="password"
              value={apiKey}
              onValueChange={setApiKey}
              autoComplete="off"
            />
          </div>
        )}

        <div className="space-y-2">
          <Label htmlFor="dsh-models">模型列表</Label>
          <Textarea
            id="dsh-models"
            value={models}
            onChange={(event) => setModels(event.target.value)}
            placeholder="deepseek-chat\ndeepseek-reasoner"
          />
          <p className="text-xs text-muted-foreground">每行一个模型 ID。</p>
        </div>

        {showButtons && (
          <div className="flex justify-end gap-2">
            <Button type="button" variant="outline" onClick={onCancel}>
              {t("common.cancel")}
            </Button>
            <Button type="submit">{submitLabel}</Button>
          </div>
        )}
      </form>
    </Form>
  );
}
