import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProviderCenterPanel } from "@/components/provider-center/ProviderCenterPanel";
import type {
  ImportCandidate,
  ProviderImportSession,
} from "@/lib/api/providerCenter";

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
}));

vi.mock("sonner", () => ({
  toast: toastMock,
}));

const settingsApiMock = vi.hoisted(() => ({
  getToolVersions: vi.fn(),
  getDesktopAppStatus: vi.fn(),
}));

vi.mock("@/lib/api/settings", () => ({
  settingsApi: settingsApiMock,
}));

const providerCenterApiMock = vi.hoisted(() => ({
  get: vi.fn(),
  getModelCatalog: vi.fn(),
  startImportSession: vi.fn(),
  getImportSession: vi.fn(),
  commitImportCandidate: vi.fn(),
  importCandidate: vi.fn(),
  save: vi.fn(),
  delete: vi.fn(),
  duplicate: vi.fn(),
  discoverModels: vi.fn(),
  previewApply: vi.fn(),
  applyTransaction: vi.fn(),
  restoreTransaction: vi.fn(),
  setOverride: vi.fn(),
  disableBinding: vi.fn(),
  scanImports: vi.fn(),
}));

vi.mock("@/lib/api/providerCenter", () => ({
  providerCenterApi: providerCenterApiMock,
}));

const candidate = (overrides: Partial<ImportCandidate>): ImportCandidate => ({
  id: "candidate-1",
  sessionId: "session-1",
  sourceRef: "saved:codex:src-1",
  sourceApp: "codex",
  sourceKind: "localManaged",
  name: "Imported Provider",
  protocol: "openai-responses",
  baseUrl: "https://clean.example/v1",
  models: ["gpt-5"],
  credentialConfigured: true,
  credentialHint: "…1234",
  conflicts: [],
  ...overrides,
});

const session = (candidates: ImportCandidate[]): ProviderImportSession => ({
  id: "session-1",
  state: "ready",
  candidates,
  errors: [],
  createdAt: 1,
  expiresAt: 2,
});

const conflicted = candidate({
  id: "candidate-2",
  sourceRef: "saved:codex:src-2",
  name: "Duplicate Provider",
  baseUrl: "https://dup.example/v1",
  conflicts: [
    {
      existingProviderId: "shared-1",
      existingName: "Existing Shared",
      existingRevision: 2,
      reasons: ["sameEndpoint"],
    },
  ],
});

const renderPanel = () => {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <ProviderCenterPanel />
    </QueryClientProvider>,
  );
};

const openImportDialog = async () => {
  const buttons = await screen.findAllByText("providerCenter.importLocal");
  await userEvent.click(buttons[0]);
  expect(providerCenterApiMock.startImportSession).toHaveBeenCalled();
};

const candidateSection = (baseUrl: string) => {
  const section = screen.getByText(baseUrl).closest("section");
  expect(section).not.toBeNull();
  return within(section as HTMLElement);
};

describe("ProviderCenterPanel import flow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    providerCenterApiMock.get.mockResolvedValue({
      definitions: [],
      bindings: [],
      transactions: [],
    });
    providerCenterApiMock.getModelCatalog.mockResolvedValue({ entries: [] });
    settingsApiMock.getToolVersions.mockResolvedValue([
      {
        name: "codex",
        version: "1.0.0",
        latest_version: null,
        error: null,
        installed_but_broken: false,
      },
    ]);
    settingsApiMock.getDesktopAppStatus.mockResolvedValue(null);
    providerCenterApiMock.startImportSession.mockResolvedValue(
      session([candidate({}), conflicted]),
    );
    providerCenterApiMock.getImportSession.mockResolvedValue(session([]));
    providerCenterApiMock.commitImportCandidate.mockResolvedValue({
      action: "createCopy",
      repeated: false,
    });
  });

  it("commits conflicted candidates as an explicit copy without overwriting", async () => {
    renderPanel();
    await openImportDialog();

    const section = candidateSection("https://dup.example/v1");
    await userEvent.click(section.getByText("providerCenter.import.copy"));

    expect(providerCenterApiMock.commitImportCandidate).toHaveBeenCalledWith(
      "session-1",
      "candidate-2",
      ["codex"],
      { action: "createCopy" },
    );
  });

  it("merges only into the scanned conflict revision", async () => {
    renderPanel();
    await openImportDialog();

    const section = candidateSection("https://dup.example/v1");
    await userEvent.click(
      section.getByText("providerCenter.import.actionMerge"),
    );
    await userEvent.click(
      section.getByText("providerCenter.import.confirmMerge"),
    );

    expect(providerCenterApiMock.commitImportCandidate).toHaveBeenCalledWith(
      "session-1",
      "candidate-2",
      ["codex"],
      {
        action: "merge",
        targetProviderId: "shared-1",
        expectedRevision: 2,
      },
    );
  });

  it("skips a candidate without creating app bindings", async () => {
    renderPanel();
    await openImportDialog();

    const section = candidateSection("https://dup.example/v1");
    await userEvent.click(
      section.getByText("providerCenter.import.actionSkip"),
    );
    await userEvent.click(
      section.getByText("providerCenter.import.confirmSkip"),
    );

    expect(providerCenterApiMock.commitImportCandidate).toHaveBeenCalledWith(
      "session-1",
      "candidate-2",
      [],
      { action: "skip" },
    );
  });

  it("commits clean candidates with an explicit createCopy decision", async () => {
    renderPanel();
    await openImportDialog();

    const section = candidateSection("https://clean.example/v1");
    await userEvent.click(section.getByText("providerCenter.import.copy"));

    expect(providerCenterApiMock.commitImportCandidate).toHaveBeenCalledWith(
      "session-1",
      "candidate-1",
      ["codex"],
      {
        action: "createCopy",
      },
    );
  });
});
