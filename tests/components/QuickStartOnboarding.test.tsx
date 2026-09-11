import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QuickStartOnboarding } from "@/components/onboarding/QuickStartOnboarding";
import type { Settings } from "@/types";
import { createTestQueryClient } from "../utils/testQueryClient";

const settingsApiMock = vi.hoisted(() => ({
  get: vi.fn(),
  save: vi.fn(),
  getToolVersions: vi.fn(),
  runToolLifecycleAction: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  settingsApi: settingsApiMock,
}));

const providerCenterApiMock = vi.hoisted(() => ({
  save: vi.fn(),
  previewApply: vi.fn(),
  applyTransaction: vi.fn(),
}));

vi.mock("@/lib/api/providerCenter", () => ({
  providerCenterApi: providerCenterApiMock,
}));

const renderOnboarding = (onComplete = vi.fn()) => {
  const queryClient = createTestQueryClient();
  render(
    <QueryClientProvider client={queryClient}>
      <QuickStartOnboarding onComplete={onComplete} />
    </QueryClientProvider>,
  );
  return onComplete;
};

describe("QuickStartOnboarding", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    settingsApiMock.get.mockResolvedValue({
      quickStartOnboardingCompleted: false,
    } as unknown as Settings);
    settingsApiMock.getToolVersions.mockResolvedValue([
      { name: "codex", version: "0.50.0", installed_but_broken: false },
    ]);
  });

  it("stays hidden once the onboarding was completed", async () => {
    settingsApiMock.get.mockResolvedValue({
      quickStartOnboardingCompleted: true,
    } as unknown as Settings);

    renderOnboarding();

    const title = await screen.findByText("quickStart.title");
    expect(title.closest("[aria-hidden]")).toHaveAttribute(
      "aria-hidden",
      "true",
    );
  });

  it("marks the onboarding as done when skipped without saving a provider", async () => {
    const onComplete = renderOnboarding();

    await screen.findByText("quickStart.title");
    await userEvent.click(screen.getAllByText("quickStart.skip")[0]);

    await vi.waitFor(() => {
      expect(settingsApiMock.save).toHaveBeenCalledWith(
        expect.objectContaining({ quickStartOnboardingCompleted: true }),
      );
    });
    expect(onComplete).toHaveBeenCalled();
    expect(providerCenterApiMock.save).not.toHaveBeenCalled();
  });

  it("blocks saving until name, base URL and API key are provided", async () => {
    renderOnboarding();

    await screen.findByText("quickStart.title");
    await userEvent.click(screen.getAllByText("quickStart.saveAndStart")[0]);

    expect(
      await screen.findByText("quickStart.requiredFields"),
    ).toBeInTheDocument();
    expect(providerCenterApiMock.save).not.toHaveBeenCalled();
  });
});
