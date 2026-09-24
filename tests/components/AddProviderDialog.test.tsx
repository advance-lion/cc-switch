import {
  act,
  fireEvent,
  render as testingRender,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useEffect, type ReactElement, type ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AddProviderDialog } from "@/components/providers/AddProviderDialog";
import { providerCenterApi } from "@/lib/api/providerCenter";
import type { ProviderApplyPreview } from "@/lib/api/providerCenter";
import type { ProviderFormValues } from "@/components/providers/forms/ProviderForm";
import { codexProviderPresets } from "@/config/codexProviderPresets";

const toastMocks = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
}));

vi.mock("sonner", () => ({ toast: toastMocks }));

vi.mock("@/lib/api/providerCenter", () => ({
  providerCenterApi: {
    get: vi.fn().mockResolvedValue({
      definitions: [],
      bindings: [],
      transactions: [],
    }),
    save: vi.fn(),
    previewApply: vi.fn(),
    applyTransaction: vi.fn(),
    startImportSession: vi.fn(),
    commitImportCandidate: vi.fn(),
    previewManagedDraft: vi.fn(),
    confirmManagedDraft: vi.fn(),
  },
}));

vi.mock("@/components/ui/dialog", () => ({
  Dialog: ({ children, open }: { children: React.ReactNode; open?: boolean }) =>
    open ? <div>{children}</div> : null,
  DialogContent: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogHeader: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogTitle: ({ children }: { children: React.ReactNode }) => (
    <h1>{children}</h1>
  ),
  DialogDescription: ({ children }: { children: React.ReactNode }) => (
    <p>{children}</p>
  ),
  DialogFooter: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}));

let mockFormValues: ProviderFormValues;
let mockFormReady = true;
let submitReadyCallbacks: Array<(isReady: boolean) => void> = [];

const render = (ui: ReactElement) => {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  return testingRender(ui, { wrapper: Wrapper });
};

vi.mock("@/components/providers/forms/ProviderForm", () => ({
  ProviderForm: ({
    onSubmit,
    onSubmitReadyChange,
    onManageAuthAccounts,
  }: {
    onSubmit: (values: ProviderFormValues) => void;
    onSubmitReadyChange?: (isReady: boolean) => void;
    onManageAuthAccounts?: (target: "codex_oauth") => void;
  }) => {
    useEffect(() => {
      if (onSubmitReadyChange) {
        submitReadyCallbacks.push(onSubmitReadyChange);
        onSubmitReadyChange(mockFormReady);
      }
    }, [onSubmitReadyChange]);
    return (
      <form
        id="provider-form"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(mockFormValues);
        }}
      >
        <button
          type="button"
          onClick={() => onManageAuthAccounts?.("codex_oauth")}
        >
          manage-auth
        </button>
      </form>
    );
  },
}));

vi.mock("@/components/providers/AuthSettingsPanel", () => ({
  AuthSettingsPanel: ({ target }: { target: string | null }) =>
    target ? <div data-testid="auth-settings-panel">{target}</div> : null,
}));

vi.mock("@/components/ConfirmDialog", () => ({
  ConfirmDialog: ({ isOpen, onConfirm, confirmText }: any) =>
    isOpen ? (
      <div>
        <button onClick={() => onConfirm(false)}>{confirmText}</button>
      </div>
    ) : null,
}));

describe("AddProviderDialog", () => {
  const selectAgentOnly = () => {
    const btn = screen.queryByRole("button", { name: "仅当前 Agent" });
    if (btn) fireEvent.click(btn);
  };

  beforeEach(() => {
    vi.mocked(providerCenterApi.get).mockResolvedValue({
      definitions: [],
      bindings: [],
      transactions: [],
    });
    vi.mocked(providerCenterApi.save).mockReset();
    vi.mocked(providerCenterApi.previewApply).mockReset();
    vi.mocked(providerCenterApi.applyTransaction).mockReset();
    vi.mocked(providerCenterApi.previewManagedDraft).mockReset();
    vi.mocked(providerCenterApi.confirmManagedDraft).mockReset();
    mockFormReady = true;
    submitReadyCallbacks = [];
    mockFormValues = {
      name: "Test Provider",
      websiteUrl: "https://provider.example.com",
      settingsConfig: JSON.stringify({ env: {}, config: {} }),
      meta: {
        custom_endpoints: {
          "https://api.new-endpoint.com": {
            url: "https://api.new-endpoint.com",
            addedAt: 1,
          },
        },
      },
    };
  });

  it("defaults to agent-only save scope", async () => {
    render(
      <AddProviderDialog
        open
        onOpenChange={vi.fn()}
        appId="codex"
        onSubmit={vi.fn()}
      />,
    );

    // Save-scope toggle is visible
    expect(
      screen.getByRole("button", { name: "仅当前 Agent" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "通用 Provider" }),
    ).toBeInTheDocument();
    // Submit button uses common.add label (agent-only default, not universal)
    expect(
      screen.getByRole("button", { name: "common.add" }),
    ).toBeInTheDocument();
  });

  it("calls previewManagedDraft (no writes) when switching to universal scope and submitting", async () => {
    const onOpenChange = vi.fn();
    vi.mocked(providerCenterApi.previewManagedDraft).mockResolvedValue({
      token: "preview-token",
      providerId: "shared-provider",
      providerRevision: 1,
      createdAt: 1,
      targets: [
        {
          appType: "codex",
          operation: "create",
          compatible: true,
          connectionMode: "direct",
          requiresTakeover: false,
          drifted: false,
        },
      ],
    });

    render(
      <AddProviderDialog
        open
        onOpenChange={onOpenChange}
        appId="codex"
        onSubmit={vi.fn()}
      />,
    );

    // Switch to universal scope
    fireEvent.click(
      screen.getByRole("button", { name: "通用 Provider" }),
    );

    // Submit the form (ProviderForm is mocked, triggers onSubmit handler)
    fireEvent.click(
      screen.getByRole("button", { name: /universalProvider\.add/ }),
    );

    // Preview is generated, no confirm yet
    await waitFor(() =>
      expect(providerCenterApi.previewManagedDraft).toHaveBeenCalledWith(
        expect.objectContaining({ appType: "codex" }),
      ),
    );
    // confirmManagedDraft must NOT be called before user confirms
    expect(providerCenterApi.confirmManagedDraft).not.toHaveBeenCalled();
  });

  it("does not report success or close when the managed transaction rolls back", async () => {
    const onOpenChange = vi.fn();
    const preview: ProviderApplyPreview = {
      token: "preview-token",
      providerId: "shared-provider",
      providerRevision: 1,
      createdAt: 1,
      targets: [
        {
          appType: "codex",
          operation: "create",
          compatible: true,
          connectionMode: "direct",
          requiresTakeover: false,
          drifted: false,
        },
      ],
    };
    vi.mocked(providerCenterApi.previewManagedDraft).mockResolvedValue(preview);
    vi.mocked(providerCenterApi.confirmManagedDraft).mockResolvedValue({
      id: "failed-transaction",
      providerId: "shared-provider",
      providerRevision: 1,
      status: "rolled_back",
      createdAt: 1,
      completedAt: 2,
      targets: [
        {
          appType: "codex",
          status: "rolled_back",
          message: "写入后配置校验失败",
        },
      ],
    });

    render(
      <AddProviderDialog
        open
        onOpenChange={onOpenChange}
        appId="codex"
        onSubmit={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "通用 Provider" }));
    fireEvent.click(
      screen.getByRole("button", { name: /universalProvider\.add/ }),
    );
    await screen.findByRole("button", { name: "预览并继续" });
    fireEvent.click(screen.getByRole("button", { name: "预览并继续" }));
    await screen.findByRole("button", { name: "确认添加" });
    fireEvent.click(screen.getByRole("button", { name: "确认添加" }));

    await waitFor(() =>
      expect(toastMocks.error).toHaveBeenCalledWith("写入后配置校验失败"),
    );
    expect(toastMocks.success).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });

  it("使用 ProviderForm 返回的自定义端点", async () => {
    const handleSubmit = vi.fn().mockResolvedValue(undefined);
    const handleOpenChange = vi.fn();

    render(
      <AddProviderDialog
        open
        onOpenChange={handleOpenChange}
        appId="claude"
        onSubmit={handleSubmit}
      />,
    );

    selectAgentOnly();
    fireEvent.click(
      screen.getByRole("button", {
        name: "common.add",
      }),
    );

    await waitFor(() => expect(handleSubmit).toHaveBeenCalledTimes(1));

    const submitted = handleSubmit.mock.calls[0][0];
    expect(submitted.meta?.custom_endpoints).toEqual(
      mockFormValues.meta?.custom_endpoints,
    );
    expect(handleOpenChange).toHaveBeenCalledWith(false);
  });

  it("在缺少自定义端点时回退到配置中的 baseUrl", async () => {
    const handleSubmit = vi.fn().mockResolvedValue(undefined);

    mockFormValues = {
      name: "Base URL Provider",
      websiteUrl: "",
      settingsConfig: JSON.stringify({
        env: { ANTHROPIC_BASE_URL: "https://claude.base" },
        config: {},
      }),
    };

    render(
      <AddProviderDialog
        open
        onOpenChange={vi.fn()}
        appId="claude"
        onSubmit={handleSubmit}
      />,
    );

    selectAgentOnly();
    fireEvent.click(
      screen.getByRole("button", {
        name: "common.add",
      }),
    );

    await waitFor(() => expect(handleSubmit).toHaveBeenCalledTimes(1));

    const submitted = handleSubmit.mock.calls[0][0];
    expect(submitted.meta?.custom_endpoints).toEqual({
      "https://claude.base": {
        url: "https://claude.base",
        addedAt: expect.any(Number),
        lastUsed: undefined,
      },
    });
  });

  it("submits the optional managed account from the Codex Official preset", async () => {
    const handleSubmit = vi.fn().mockResolvedValue(undefined);
    const officialPresetIndex = codexProviderPresets.findIndex(
      (preset) =>
        preset.category === "official" && preset.providerType === "codex_oauth",
    );
    expect(officialPresetIndex).toBeGreaterThanOrEqual(0);

    mockFormValues = {
      name: "OpenAI Official",
      websiteUrl: "https://chatgpt.com/codex",
      settingsConfig: JSON.stringify({ auth: {}, config: "" }),
      presetId: `codex-${officialPresetIndex}`,
      presetCategory: "official",
      meta: {
        providerType: "codex_oauth",
        authBinding: {
          source: "managed_account",
          authProvider: "codex_oauth",
          accountId: "acct-managed",
        },
      },
    };

    render(
      <AddProviderDialog
        open
        onOpenChange={vi.fn()}
        appId="codex"
        onSubmit={handleSubmit}
      />,
    );

    selectAgentOnly();
    fireEvent.click(screen.getByRole("button", { name: "common.add" }));

    await waitFor(() => expect(handleSubmit).toHaveBeenCalledTimes(1));
    expect(handleSubmit).toHaveBeenCalledWith(
      expect.objectContaining({
        category: "official",
        meta: expect.objectContaining({
          authBinding: {
            source: "managed_account",
            authProvider: "codex_oauth",
            accountId: "acct-managed",
          },
        }),
      }),
    );
    expect(handleSubmit.mock.calls[0][0]).not.toHaveProperty(
      "ensureCodexOfficialSeed",
    );
  });

  it("clears the nested auth panel before the dialog reopens", async () => {
    const props = {
      onOpenChange: vi.fn(),
      appId: "codex" as const,
      onSubmit: vi.fn(),
    };
    const { rerender } = render(<AddProviderDialog open {...props} />);

    selectAgentOnly();
    fireEvent.click(screen.getByRole("button", { name: "manage-auth" }));
    expect(screen.getByTestId("auth-settings-panel")).toHaveTextContent(
      "codex_oauth",
    );

    rerender(<AddProviderDialog open={false} {...props} />);
    rerender(<AddProviderDialog open {...props} />);

    await waitFor(() => {
      expect(
        screen.queryByTestId("auth-settings-panel"),
      ).not.toBeInTheDocument();
    });
  });

  it("新建 Grok Build 自定义供应商时不补默认 Grok 图标", async () => {
    const handleSubmit = vi.fn().mockResolvedValue(undefined);

    mockFormValues = {
      name: "tes 1",
      websiteUrl: "",
      icon: "",
      iconColor: "",
      settingsConfig: JSON.stringify({
        config: `[models]
default = "grok-4.5"

[model."grok-4.5"]
model = "grok-4.5"
base_url = "https://grok.example.com/v1"
name = "tes 1"
api_key = "secret"
api_backend = "responses"
context_window = 500000
`,
      }),
    };

    render(
      <AddProviderDialog
        open
        onOpenChange={vi.fn()}
        appId="grokbuild"
        onSubmit={handleSubmit}
      />,
    );

    selectAgentOnly();
    fireEvent.click(screen.getByRole("button", { name: "common.add" }));

    await waitFor(() => expect(handleSubmit).toHaveBeenCalledTimes(1));

    const submitted = handleSubmit.mock.calls[0][0];
    expect(submitted.icon).toBeUndefined();
    expect(submitted.iconColor).toBeUndefined();
  });

  it("Pi 添加供应商时仅提交供应商目录", async () => {
    const handleSubmit = vi.fn().mockResolvedValue(undefined);
    mockFormValues = {
      name: "Pi Provider",
      providerKey: "pi-provider",
      websiteUrl: "",
      settingsConfig: JSON.stringify({
        baseUrl: "https://api.example.com/v1",
        models: [
          { id: "selected-model", name: "Selected" },
          { id: "other-model", name: "Other" },
        ],
      }),
      meta: {
        isPartner: true,
        endpointAutoSelect: true,
        custom_endpoints: {
          "https://failover.example.com/v1": {
            url: "https://failover.example.com/v1",
            addedAt: 1,
          },
        },
      },
    };

    render(
      <AddProviderDialog
        open
        onOpenChange={vi.fn()}
        appId="pi"
        onSubmit={handleSubmit}
      />,
    );

    selectAgentOnly();
    fireEvent.click(screen.getByRole("button", { name: "common.add" }));
    await waitFor(() => expect(handleSubmit).toHaveBeenCalledTimes(1));
    expect(handleSubmit.mock.calls[0][0]).toMatchObject({
      providerKey: "pi-provider",
      meta: { isPartner: true },
    });
    expect(handleSubmit.mock.calls[0][0]).not.toHaveProperty(
      "piActivateModelId",
    );
  });

  it("重新打开 Pi 表单后忽略上一轮的就绪回调", async () => {
    const props = {
      onOpenChange: vi.fn(),
      appId: "pi" as const,
      onSubmit: vi.fn(),
    };
    const { rerender } = render(<AddProviderDialog open {...props} />);

    selectAgentOnly();
    const addButton = await screen.findByRole("button", { name: "common.add" });
    await waitFor(() => expect(addButton).toBeEnabled());
    const staleCallback = submitReadyCallbacks.at(-1);
    expect(staleCallback).toBeDefined();

    rerender(<AddProviderDialog open={false} {...props} />);
    mockFormReady = false;
    rerender(<AddProviderDialog open {...props} />);
    selectAgentOnly();
    const reopenedButton = await screen.findByRole("button", {
      name: "common.add",
    });
    await waitFor(() => expect(reopenedButton).toBeDisabled());

    act(() => staleCallback?.(true));
    expect(reopenedButton).toBeDisabled();
  });
});
