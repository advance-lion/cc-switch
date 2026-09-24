import { beforeEach, describe, expect, it, vi } from "vitest";

const settingsApiMock = vi.hoisted(() => ({
  getToolVersions: vi.fn(),
}));

vi.mock("@/lib/api", () => ({ settingsApi: settingsApiMock }));

import { discoverManagedAgents } from "@/lib/managedAgents";

describe("discoverManagedAgents", () => {
  beforeEach(() => vi.clearAllMocks());

  it("does not duplicate first-class Provider Agents in the lifecycle-only registry", async () => {
    settingsApiMock.getToolVersions.mockResolvedValue([
      {
        name: "qoder",
        version: "1.1.62",
        latest_version: null,
        error: null,
        installed_but_broken: false,
      },
    ]);

    await expect(discoverManagedAgents()).resolves.toEqual([]);
    expect(settingsApiMock.getToolVersions).toHaveBeenCalledWith([]);
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
