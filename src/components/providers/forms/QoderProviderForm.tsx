import { useEffect, useMemo, useState } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Form, FormField } from "@/components/ui/form";
import { ImeSafeInput } from "@/components/ui/ime-safe-input";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { BasicFormFields } from "./BasicFormFields";
import type { ProviderFormProps, ProviderFormValues } from "./ProviderForm";
import { providerSchema, type ProviderFormData } from "@/lib/schemas/provider";

type QoderProtocol = "openai" | "openai-responses" | "anthropic";

interface QoderModelDraft {
  key: string;
  model: string;
  displayName: string;
  contextWindow: string;
  maxOutputTokens: string;
  tools: boolean;
  vision: boolean;
  passthrough: Record<string, unknown>;
}

const ROOT_KEYS = new Set([
  "type",
  "displayName",
  "protocol",
  "authType",
  "baseUrl",
  "apiKey",
  "model",
  "models",
]);
const MODEL_KEYS = new Set([
  "model",
  "displayName",
  "contextWindow",
  "maxOutputTokens",
  "capabilities",
]);

const asObject = (value: unknown): Record<string, unknown> =>
  value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

const withoutKeys = (
  value: Record<string, unknown>,
  keys: Set<string>,
): Record<string, unknown> =>
  Object.fromEntries(Object.entries(value).filter(([key]) => !keys.has(key)));

const text = (value: unknown) => (typeof value === "string" ? value : "");
const numberText = (value: unknown) =>
  typeof value === "number" && Number.isFinite(value) ? String(value) : "";

function toModelDraft(value: unknown): QoderModelDraft {
  const model = asObject(value);
  const capabilities = asObject(model.capabilities);
  return {
    key: crypto.randomUUID(),
    model: text(model.model),
    displayName: text(model.displayName),
    contextWindow: numberText(model.contextWindow),
    maxOutputTokens: numberText(model.maxOutputTokens),
    tools: capabilities.tools !== false,
    vision: capabilities.vision === true,
    passthrough: withoutKeys(model, MODEL_KEYS),
  };
}

function emptyModel(): QoderModelDraft {
  return {
    key: crypto.randomUUID(),
    model: "",
    displayName: "",
    contextWindow: "",
    maxOutputTokens: "",
    tools: true,
    vision: false,
    passthrough: {},
  };
}

const positiveInteger = (value: string): number | undefined => {
  if (!value.trim()) return undefined;
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
};

export function QoderProviderForm({
  providerId,
  submitLabel,
  onSubmit,
  onCancel,
  onSubmittingChange,
  onSubmitReadyChange,
  initialData,
  showButtons = true,
}: ProviderFormProps) {
  const initialConfig = useMemo(
    () => asObject(initialData?.settingsConfig),
    [initialData?.settingsConfig],
  );
  const initialModels = useMemo(() => {
    const models = Array.isArray(initialConfig.models)
      ? initialConfig.models.map(toModelDraft)
      : [];
    return models.length > 0 ? models : [emptyModel()];
  }, [initialConfig.models]);
  const [providerKey, setProviderKey] = useState(providerId ?? "");
  const [protocol, setProtocol] = useState<QoderProtocol>(() => {
    const value = text(initialConfig.protocol);
    return value === "openai-responses" || value === "anthropic"
      ? value
      : "openai";
  });
  const [baseUrl, setBaseUrl] = useState(text(initialConfig.baseUrl));
  const [apiKey, setApiKey] = useState(text(initialConfig.apiKey));
  const [defaultModel, setDefaultModel] = useState(text(initialConfig.model));
  const [models, setModels] = useState<QoderModelDraft[]>(initialModels);
  const [formError, setFormError] = useState<string | null>(null);
  const rootPassthrough = useMemo(
    () => withoutKeys(initialConfig, ROOT_KEYS),
    [initialConfig],
  );

  const form = useForm<ProviderFormData>({
    resolver: zodResolver(providerSchema),
    defaultValues: {
      name: initialData?.name ?? text(initialConfig.displayName),
      websiteUrl: initialData?.websiteUrl ?? "",
      notes: initialData?.notes ?? "",
      settingsConfig: JSON.stringify(initialConfig),
      icon: initialData?.icon ?? "qoder",
      iconColor: initialData?.iconColor ?? "",
    },
  });
  const isSubmitting = form.formState.isSubmitting;
  const ready =
    providerKey.trim().length > 0 &&
    baseUrl.trim().length > 0 &&
    models.some((model) => model.model.trim().length > 0);

  useEffect(
    () => onSubmittingChange?.(isSubmitting),
    [isSubmitting, onSubmittingChange],
  );
  useEffect(() => onSubmitReadyChange?.(ready), [ready, onSubmitReadyChange]);

  const updateModel = (key: string, patch: Partial<QoderModelDraft>) => {
    setModels((current) =>
      current.map((model) =>
        model.key === key ? { ...model, ...patch } : model,
      ),
    );
  };

  const handleSubmit = form.handleSubmit(async (values) => {
    const normalizedKey = providerKey.trim().toLowerCase();
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(normalizedKey)) {
      setFormError("Provider 标识只能包含小写字母、数字和连字符");
      return;
    }
    let parsedUrl: URL;
    try {
      parsedUrl = new URL(baseUrl.trim());
    } catch {
      setFormError("请输入有效的 HTTP 或 HTTPS Base URL");
      return;
    }
    if (!["http:", "https:"].includes(parsedUrl.protocol)) {
      setFormError("请输入有效的 HTTP 或 HTTPS Base URL");
      return;
    }

    const normalizedModels = models
      .filter((model) => model.model.trim())
      .map((model) => ({
        ...model.passthrough,
        model: model.model.trim(),
        displayName: model.displayName.trim() || model.model.trim(),
        ...(positiveInteger(model.contextWindow)
          ? { contextWindow: positiveInteger(model.contextWindow) }
          : {}),
        ...(positiveInteger(model.maxOutputTokens)
          ? { maxOutputTokens: positiveInteger(model.maxOutputTokens) }
          : {}),
        capabilities: { tools: model.tools, vision: model.vision },
      }));
    if (normalizedModels.length === 0) {
      setFormError("至少添加一个模型");
      return;
    }
    const selectedModel = normalizedModels.some(
      (model) => model.model === defaultModel.trim(),
    )
      ? defaultModel.trim()
      : normalizedModels[0].model;
    const config = {
      ...rootPassthrough,
      type: "openai-compatible",
      displayName: values.name.trim(),
      protocol,
      authType: "bearer",
      baseUrl: baseUrl.trim().replace(/\/+$/, ""),
      apiKey,
      model: selectedModel,
      models: normalizedModels,
    };
    setFormError(null);
    const payload: ProviderFormValues = {
      ...values,
      settingsConfig: JSON.stringify(config, null, 2),
      providerKey: normalizedKey,
      presetCategory: initialData?.category ?? "custom",
    };
    await onSubmit(payload);
  });

  return (
    <Form {...form}>
      <form id="provider-form" onSubmit={handleSubmit} className="space-y-6">
        <BasicFormFields
          form={form}
          beforeNameSlot={
            <div className="space-y-2">
              <Label htmlFor="qoder-provider-key">Provider 标识</Label>
              <ImeSafeInput
                id="qoder-provider-key"
                value={providerKey}
                onValueChange={(value) =>
                  setProviderKey(value.toLowerCase().replace(/[^a-z0-9-]/g, ""))
                }
                disabled={Boolean(providerId)}
                placeholder="my-provider"
              />
              <p className="text-xs text-muted-foreground">
                写入 Qoder settings.json 的 Provider 键；启用后不可改名。
              </p>
            </div>
          }
        />

        <div className="grid gap-4 md:grid-cols-2">
          <div className="space-y-2">
            <Label>协议</Label>
            <Select
              value={protocol}
              onValueChange={(value) => setProtocol(value as QoderProtocol)}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="openai">OpenAI Chat Completions</SelectItem>
                <SelectItem value="openai-responses">
                  OpenAI Responses
                </SelectItem>
                <SelectItem value="anthropic">Anthropic Messages</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-2">
            <Label htmlFor="qoder-base-url">Base URL</Label>
            <Input
              id="qoder-base-url"
              value={baseUrl}
              onChange={(event) => setBaseUrl(event.target.value)}
              placeholder="https://api.example.com/v1"
            />
          </div>
        </div>

        <div className="space-y-2">
          <Label htmlFor="qoder-api-key">API Key</Label>
          <Input
            id="qoder-api-key"
            type="password"
            autoComplete="off"
            value={apiKey}
            onChange={(event) => setApiKey(event.target.value)}
            placeholder={initialData ? "留空则保存为空" : "sk-..."}
          />
        </div>

        <fieldset className="space-y-3 rounded-xl border border-border/70 p-4">
          <div className="flex items-center justify-between gap-3">
            <div>
              <legend className="text-sm font-medium">模型</legend>
              <p className="mt-0.5 text-xs text-muted-foreground">
                Qoder 原生支持一个 Provider 下维护多个模型。
              </p>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setModels((current) => [...current, emptyModel()])}
            >
              <Plus className="mr-1.5 h-3.5 w-3.5" />
              添加模型
            </Button>
          </div>

          <div className="space-y-3">
            {models.map((model, index) => (
              <div
                key={model.key}
                className="rounded-lg border bg-muted/15 p-3"
              >
                <div className="mb-3 flex items-center justify-between">
                  <span className="text-xs font-medium">模型 {index + 1}</span>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-muted-foreground hover:text-destructive"
                    disabled={models.length === 1}
                    onClick={() =>
                      setModels((current) =>
                        current.filter((item) => item.key !== model.key),
                      )
                    }
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </div>
                <div className="grid gap-3 md:grid-cols-2">
                  <Field label="模型 ID" htmlFor={`qoder-model-${model.key}`}>
                    <Input
                      id={`qoder-model-${model.key}`}
                      value={model.model}
                      onChange={(event) =>
                        updateModel(model.key, { model: event.target.value })
                      }
                      placeholder="model-id"
                    />
                  </Field>
                  <Field
                    label="显示名称"
                    htmlFor={`qoder-model-name-${model.key}`}
                  >
                    <Input
                      id={`qoder-model-name-${model.key}`}
                      value={model.displayName}
                      onChange={(event) =>
                        updateModel(model.key, {
                          displayName: event.target.value,
                        })
                      }
                      placeholder={model.model || "Model"}
                    />
                  </Field>
                  <Field
                    label="上下文窗口"
                    htmlFor={`qoder-context-${model.key}`}
                  >
                    <Input
                      id={`qoder-context-${model.key}`}
                      type="number"
                      min="1"
                      value={model.contextWindow}
                      onChange={(event) =>
                        updateModel(model.key, {
                          contextWindow: event.target.value,
                        })
                      }
                      placeholder="128000"
                    />
                  </Field>
                  <Field
                    label="最大输出 Token"
                    htmlFor={`qoder-output-${model.key}`}
                  >
                    <Input
                      id={`qoder-output-${model.key}`}
                      type="number"
                      min="1"
                      value={model.maxOutputTokens}
                      onChange={(event) =>
                        updateModel(model.key, {
                          maxOutputTokens: event.target.value,
                        })
                      }
                      placeholder="8192"
                    />
                  </Field>
                </div>
                <div className="mt-3 flex flex-wrap gap-5">
                  <label className="flex items-center gap-2 text-sm">
                    <Checkbox
                      checked={model.tools}
                      onCheckedChange={(checked) =>
                        updateModel(model.key, { tools: checked === true })
                      }
                    />
                    工具调用
                  </label>
                  <label className="flex items-center gap-2 text-sm">
                    <Checkbox
                      checked={model.vision}
                      onCheckedChange={(checked) =>
                        updateModel(model.key, { vision: checked === true })
                      }
                    />
                    视觉输入
                  </label>
                </div>
              </div>
            ))}
          </div>

          <div className="space-y-2">
            <Label>默认模型</Label>
            <Select
              value={defaultModel || models[0]?.model || ""}
              onValueChange={setDefaultModel}
            >
              <SelectTrigger>
                <SelectValue placeholder="选择默认模型" />
              </SelectTrigger>
              <SelectContent>
                {models
                  .filter((model) => model.model.trim())
                  .map((model) => (
                    <SelectItem key={model.key} value={model.model.trim()}>
                      {model.displayName.trim() || model.model.trim()}
                    </SelectItem>
                  ))}
              </SelectContent>
            </Select>
          </div>
        </fieldset>

        {formError && <p className="text-sm text-destructive">{formError}</p>}

        <FormField
          control={form.control}
          name="settingsConfig"
          render={({ field }) => <input type="hidden" {...field} />}
        />

        {showButtons && (
          <div className="flex justify-end gap-2">
            <Button type="button" variant="outline" onClick={onCancel}>
              取消
            </Button>
            <Button type="submit" disabled={!ready}>
              {submitLabel}
            </Button>
          </div>
        )}
      </form>
    </Form>
  );
}

function Field({
  label,
  htmlFor,
  children,
}: {
  label: string;
  htmlFor: string;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-2">
      <Label htmlFor={htmlFor}>{label}</Label>
      {children}
    </div>
  );
}
