import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RuntimeLifecycleCard } from "@/components/runtime/RuntimeLifecycleCard";
import type {
  CliLifecycleJob,
  DesktopAppStatus,
  DesktopLifecycleJob,
  ToolLifecycleCapabilities,
} from "@/lib/api";

const settingsApiMock = vi.hoisted(() => ({
  getToolVersions: vi.fn(),
  checkToolUpdates: vi.fn(),
  getToolLifecycleCapabilities: vi.fn(),
  runToolLifecycleAction: vi.fn(),
  runCliLifecycleAction: vi.fn(),
  listCliLifecycleJobs: vi.fn(),
  getCliLifecycleJob: vi.fn(),
  cancelCliLifecycleJob: vi.fn(),
  launchToolTerminal: vi.fn(),
  uninstallToolRuntime: vi.fn(),
  getDesktopAppStatus: vi.fn(),
  checkDesktopAppUpdates: vi.fn(),
  runDesktopAppLifecycleAction: vi.fn(),
  listDesktopLifecycleJobs: vi.fn(),
  getDesktopLifecycleJob: vi.fn(),
  cancelDesktopLifecycleJob: vi.fn(),
  launchDesktopApp: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  settingsApi: settingsApiMock,
}));

const desktopStatus: DesktopAppStatus = {
  id: "codex-desktop",
  display_name: "Codex Desktop",
  installed: false,
  version: null,
  latest_version: null,
  path: null,
  launch_target: null,
  package_identity: null,
  installation_source: "not_installed",
  can_install: true,
  can_update: false,
  can_uninstall: false,
  can_launch: false,
  reason: null,
  installations: [],
};

const capabilities: ToolLifecycleCapabilities = {
  name: "codex",
  can_install: true,
  can_update: false,
  can_uninstall: false,
  can_launch: true,
  installation_source: "npm",
  reason: null,
};

const job = (overrides: Partial<DesktopLifecycleJob>): DesktopLifecycleJob => ({
  id: "job-1",
  appId: "codex-desktop",
  component: "desktop",
  action: "update",
  state: "failed",
  preProbe: null,
  postProbe: null,
  errorCode: null,
  errorMessage: null,
  logs: [],
  createdAt: 1,
  startedAt: 1,
  completedAt: 2,
  ...overrides,
});

const cliJob = (overrides: Partial<CliLifecycleJob>): CliLifecycleJob => ({
  id: "cli-job-1",
  appId: "codex",
  component: "cli",
  action: "install",
  state: "running",
  preProbe: null,
  postProbe: null,
  errorCode: null,
  errorMessage: null,
  logs: [{ at: 1, level: "info", step: "executing", message: "npm install" }],
  createdAt: 1,
  startedAt: 1,
  completedAt: null,
  ...overrides,
});

describe("RuntimeLifecycleCard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    settingsApiMock.getToolVersions.mockResolvedValue([
      {
        name: "codex",
        version: "1.0.0",
        latest_version: null,
        error: null,
        installed_but_broken: false,
      },
    ]);
    settingsApiMock.getToolLifecycleCapabilities.mockResolvedValue([
      capabilities,
    ]);
    settingsApiMock.getDesktopAppStatus.mockResolvedValue(desktopStatus);
    settingsApiMock.listDesktopLifecycleJobs.mockResolvedValue([]);
    settingsApiMock.getDesktopLifecycleJob.mockResolvedValue(null);
    settingsApiMock.listCliLifecycleJobs.mockResolvedValue([]);
    settingsApiMock.getCliLifecycleJob.mockResolvedValue(null);
    settingsApiMock.cancelCliLifecycleJob.mockResolvedValue(true);
  });

  it("keeps provider configuration state separate from install state", async () => {
    render(<RuntimeLifecycleCard appId="codex" isConfigured={false} />);

    // CLI is installed…
    expect(
      await screen.findAllByText(/appLifecycle\.installed/),
    ).not.toHaveLength(0);
    // …while the provider connection badge reports not connected.
    expect(
      await screen.findByText("appLifecycle.notConnected"),
    ).toBeInTheDocument();
  });

  it("renders interrupted jobs through the i18n contract", async () => {
    settingsApiMock.listDesktopLifecycleJobs.mockResolvedValue([
      job({ state: "interrupted", action: "update" }),
    ]);

    render(<RuntimeLifecycleCard appId="codex" isConfigured />);

    expect(
      await screen.findByText(/appLifecycle\.lastJobInterrupted/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/appLifecycle\.retryAfterDetect/),
    ).toBeInTheDocument();
  });

  it("renders failed jobs with the backend error message when present", async () => {
    settingsApiMock.listDesktopLifecycleJobs.mockResolvedValue([
      job({ state: "failed", action: "install", errorMessage: "disk full" }),
    ]);

    render(<RuntimeLifecycleCard appId="codex" isConfigured />);

    expect(
      await screen.findByText(/appLifecycle\.lastJobFailed/),
    ).toBeInTheDocument();
    expect(screen.getByText(/disk full/)).toBeInTheDocument();
  });

  it("does not present a background running job as a failure banner", async () => {
    settingsApiMock.listDesktopLifecycleJobs.mockResolvedValue([
      job({ state: "running", action: "install" }),
    ]);

    render(<RuntimeLifecycleCard appId="codex" isConfigured />);

    await screen.findAllByText(/appLifecycle\.desktopDescription/);
    expect(
      screen.queryByText(/appLifecycle\.lastJobFailed/),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText(/appLifecycle\.lastJobInterrupted/),
    ).not.toBeInTheDocument();
  });

  it("restores an in-flight CLI job after remounting the card", async () => {
    settingsApiMock.listCliLifecycleJobs.mockResolvedValue([
      cliJob({ state: "running", action: "install" }),
    ]);

    render(<RuntimeLifecycleCard appId="codex" isConfigured />);

    // 进行中的任务在重新挂载后仍然展示进度横幅，按钮不再变回假空闲状态。
    expect(
      await screen.findByText("appLifecycle.jobRunning"),
    ).toBeInTheDocument();
    expect(screen.getByText(/npm install/)).toBeInTheDocument();
  });

  it("renders failed CLI jobs with the backend error message", async () => {
    settingsApiMock.listCliLifecycleJobs.mockResolvedValue([
      cliJob({ state: "failed", action: "install", errorMessage: "disk full" }),
    ]);

    render(<RuntimeLifecycleCard appId="codex" isConfigured />);

    expect(
      await screen.findByText(/appLifecycle\.lastJobFailed/),
    ).toBeInTheDocument();
    expect(screen.getByText(/disk full/)).toBeInTheDocument();
  });
});
