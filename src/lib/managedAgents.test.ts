import { beforeEach, describe, expect, it, vi } from "vitest";

const settingsApiMock = vi.hoisted(() => ({
  getToolVersions: vi.fn(),
}));

vi.mock("@/lib/api", () => ({ settingsApi: settingsApiMock }));

import { discoverManagedAgents } from "@/lib/managedAgents";

describe("discoverManagedAgents", () => {
  beforeEach(() => vi.clearAllMocks());

  it("registers an installed Qoder CLI as a managed Agent", async () => {
    settingsApiMock.getToolVersions.mockResolvedValue([
      {
        name: "qoder",
        version: "1.1.62",
        latest_version: null,
        error: null,
        installed_but_broken: false,
      },
    ]);

    await expect(discoverManagedAgents()).resolves.toEqual([
      expect.objectContaining({
        id: "qoder",
        tool: "qoder",
        name: "Qoder",
        version: "1.1.62",
        providerIntegration: "planned",
      }),
    ]);
  });

  it("does not add an Agent that is not installed", async () => {
    settingsApiMock.getToolVersions.mockResolvedValue([
      {
        name: "qoder",
        version: null,
        latest_version: null,
        error: "not installed",
        installed_but_broken: false,
      },
    ]);

    await expect(discoverManagedAgents()).resolves.toEqual([]);
  });
});
