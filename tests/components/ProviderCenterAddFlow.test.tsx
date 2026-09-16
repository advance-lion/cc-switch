import type { ComponentProps } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProviderCenterAddFlow } from "@/components/providers/ProviderCenterAddFlow";
import type {
  ImportCandidate,
  ProviderDefinition,
} from "@/lib/api/providerCenter";

const providerCenterApiMock = vi.hoisted(() => ({
  get: vi.fn(),
  save: vi.fn(),
  attach: vi.fn(),
  disableBinding: vi.fn().mockResolvedValue(undefined),
  previewApply: vi.fn(),
  applyTransaction: vi.fn(),
  startImportSession: vi.fn(),
  commitImportCandidate: vi.fn(),
}));

vi.mock("@/lib/api/providerCenter", () => ({
  providerCenterApi: providerCenterApiMock,
}));

vi.mock("@/lib/query/providerCenter", () => ({
  refreshProviderCenterApplyCaches: vi.fn().mockResolvedValue(undefined),
}));

const definition: ProviderDefinition = {
  id: "shared-codex",
  name: "Shared Codex",
  protocol: "openai-responses",
  baseUrl: "https://api.example.test/v1",
  models: ["gpt-5"],
  discoveredModels: [],
  notes: "",
  enabled: true,
  revision: 1,
  sources: [],
  credentialConfigured: true,
  createdAt: 1,
  updatedAt: 1,
};

const candidate = (overrides: Partial<ImportCandidate>): ImportCandidate => ({
  id: "candidate-local",
  sessionId: "scan-session",
  sourceRef: "saved:codex:local",
  sourceApp: "codex",
  sourceKind: "localManaged",
  name: "Local Codex",
  protocol: "openai-responses",
  baseUrl: "https://local.example.test/v1",
  models: ["gpt-5"],
  credentialConfigured: true,
  conflicts: [],
  ...overrides,
});

const renderFlow = (props: ComponentProps<typeof ProviderCenterAddFlow>) => {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <ProviderCenterAddFlow {...props} />
    </QueryClientProvider>,
  );
};

describe("ProviderCenterAddFlow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    providerCenterApiMock.get.mockResolvedValue({
      definitions: [definition],
      bindings: [],
      transactions: [],
    });
    providerCenterApiMock.attach.mockResolvedValue({
      providerId: definition.id,
      appType: "codex",
      status: "pending",
      enabled: true,
      overrideEnabled: false,
      updatedAt: 1,
    });
    providerCenterApiMock.previewApply.mockResolvedValue({
      token: "preview-token",
      providerId: definition.id,
      providerRevision: definition.revision,
      createdAt: 1,
      targets: [
        {
          appType: "codex",
          operation: "create",
          compatible: true,
          drifted: false,
        },
      ],
    });
    providerCenterApiMock.applyTransaction.mockResolvedValue({
      id: "transaction-1",
      providerId: definition.id,
      providerRevision: definition.revision,
      status: "applied",
      targets: [],
      createdAt: 1,
      completedAt: 2,
    });
  });

  it("previews and explicitly applies an existing universal Provider", async () => {
    const user = userEvent.setup();
    const onComplete = vi.fn();
    renderFlow({
      appId: "codex",
      path: "use-universal",
      onPathChange: vi.fn(),
      onComplete,
    });

    const selector = await screen.findByLabelText(
      "provider.addPaths.selectUniversal",
    );
    await user.selectOptions(selector, definition.id);
    await user.click(
      screen.getByRole("button", { name: "providerCenter.previewApply" }),
    );

    await waitFor(() =>
      expect(providerCenterApiMock.attach).toHaveBeenCalledWith(
        definition.id,
        "codex",
      ),
    );
    expect(providerCenterApiMock.previewApply).toHaveBeenCalledWith(
      definition.id,
      ["codex"],
    );
    expect(providerCenterApiMock.applyTransaction).not.toHaveBeenCalled();

    await user.click(
      screen.getByRole("button", { name: "providerCenter.preview.confirm" }),
    );

    await waitFor(() =>
      expect(providerCenterApiMock.applyTransaction).toHaveBeenCalledWith(
        definition.id,
        ["codex"],
        "preview-token",
      ),
    );
    expect(onComplete).toHaveBeenCalledOnce();
  });

  it("filters Provider Center projections from a read-only import scan", async () => {
    const user = userEvent.setup();
    const projected = candidate({
      id: "candidate-projection",
      sourceKind: "providerCenterProjection",
      name: "Projected shared provider",
      baseUrl: "https://projected.example.test/v1",
    });
    providerCenterApiMock.startImportSession.mockResolvedValue({
      id: "scan-session",
      state: "ready",
      candidates: [candidate({}), projected],
      errors: [],
      createdAt: 1,
      expiresAt: 2,
    });

    renderFlow({
      appId: "codex",
      path: "scan",
      onPathChange: vi.fn(),
      onComplete: vi.fn(),
    });

    await user.click(
      screen.getByRole("button", { name: "providerCenter.scanLocal" }),
    );

    await waitFor(() =>
      expect(providerCenterApiMock.startImportSession).toHaveBeenCalledWith([
        "codex",
      ]),
    );
    expect(await screen.findByText("Local Codex")).toBeInTheDocument();
    expect(screen.queryByText("Projected shared provider")).toBeNull();
    expect(providerCenterApiMock.commitImportCandidate).not.toHaveBeenCalled();
  });

  it("imports only after confirmation, attaches the current Agent, and opens preview", async () => {
    const user = userEvent.setup();
    const importedDefinition: ProviderDefinition = {
      ...definition,
      id: "imported-codex",
      name: "Imported Codex",
    };
    providerCenterApiMock.get
      .mockResolvedValueOnce({
        definitions: [definition],
        bindings: [],
        transactions: [],
      })
      .mockResolvedValueOnce({
        definitions: [definition, importedDefinition],
        bindings: [],
        transactions: [],
      });
    providerCenterApiMock.startImportSession.mockResolvedValue({
      id: "scan-session",
      state: "ready",
      candidates: [candidate({})],
      errors: [],
      createdAt: 1,
      expiresAt: 2,
    });
    providerCenterApiMock.commitImportCandidate.mockResolvedValue({
      action: "createCopy",
      provider: importedDefinition,
      repeated: false,
    });
    providerCenterApiMock.attach.mockResolvedValue({
      providerId: importedDefinition.id,
      appType: "codex",
      status: "pending",
      enabled: true,
      overrideEnabled: false,
      updatedAt: 2,
    });
    providerCenterApiMock.previewApply.mockResolvedValue({
      token: "import-preview-token",
      providerId: importedDefinition.id,
      providerRevision: importedDefinition.revision,
      createdAt: 2,
      targets: [
        {
          appType: "codex",
          operation: "create",
          compatible: true,
          drifted: false,
          message: "Ready to project",
        },
      ],
    });

    renderFlow({
      appId: "codex",
      path: "scan",
      onPathChange: vi.fn(),
      onComplete: vi.fn(),
    });
    await user.click(
      screen.getByRole("button", { name: "providerCenter.scanLocal" }),
    );
    expect(await screen.findByText("Local Codex")).toBeInTheDocument();
    expect(providerCenterApiMock.commitImportCandidate).not.toHaveBeenCalled();

    await user.click(
      screen.getByRole("button", { name: "provider.addPaths.importUniversal" }),
    );

    await waitFor(() =>
      expect(providerCenterApiMock.commitImportCandidate).toHaveBeenCalledWith(
        "scan-session",
        "candidate-local",
        ["codex"],
        { action: "createCopy" },
      ),
    );
    expect(providerCenterApiMock.attach).toHaveBeenCalledWith(
      importedDefinition.id,
      "codex",
    );
    expect(providerCenterApiMock.previewApply).toHaveBeenCalledWith(
      importedDefinition.id,
      ["codex"],
    );
    expect(await screen.findByText("Ready to project")).toBeInTheDocument();
    expect(providerCenterApiMock.applyTransaction).not.toHaveBeenCalled();
  });
});
