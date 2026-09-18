import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RuntimeLifecycleCard } from "@/components/runtime/RuntimeLifecycleCard";
import type {
  CliLifecycleJob,
  DesktopAppStatus,
  DesktopLifecycleJob,
  ToolLifecycleCapabilities,
} from "@/lib/api";

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
}));

vi.mock("sonner", () => ({
  toast: toastMock,
}));

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
  launchDsh: vi.fn(),
  restartDsh: vi.fn(),
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
    settingsApiMock.launchDsh.mockResolvedValue(undefined);
    settingsApiMock.restartDsh.mockResolvedValue(undefined);
  });

  it("keeps a sticky app summary while lifecycle details can be collapsed", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <RuntimeLifecycleCard appId="codex" isConfigured={false} />,
    );

    const lifecycleCard = container.querySelector("section");
    expect(lifecycleCard).toHaveClass("sticky", "top-0", "z-20");

    const trigger = screen.getByRole("button", {
      name: "appLifecycle.collapseDetails",
    });
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(
      await screen.findByText("appLifecycle.cliDescription"),
    ).toBeVisible();

    await user.click(trigger);

    expect(
      screen.getByRole("button", { name: "appLifecycle.expandDetails" }),
    ).toHaveAttribute("aria-expanded", "false");
    expect(
      screen.queryByText("appLifecycle.cliDescription"),
    ).not.toBeInTheDocument();
    expect(screen.getByText("appLifecycle.title")).toBeVisible();
    expect(screen.getByText("appLifecycle.notConnected")).toBeVisible();
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

  it("starts CLI installs through the fixed tool API with a generated job id", async () => {
    const user = userEvent.setup();
    settingsApiMock.getToolVersions
      .mockResolvedValueOnce([
        {
          name: "gemini",
          version: null,
          latest_version: null,
          error: null,
          installed_but_broken: false,
        },
      ])
      .mockResolvedValueOnce([
        {
          name: "gemini",
          version: "1.0.0",
          latest_version: null,
          error: null,
          installed_but_broken: false,
        },
      ]);
    settingsApiMock.runCliLifecycleAction.mockResolvedValue(undefined);

    render(<RuntimeLifecycleCard appId="gemini" isConfigured />);

    await user.click(
      await screen.findByRole("button", {
        name: "appLifecycle.standardInstall",
      }),
    );

    await waitFor(() =>
      expect(settingsApiMock.runCliLifecycleAction).toHaveBeenCalledWith(
        "gemini",
        "install",
        expect.any(String),
      ),
    );
  });

  it("starts CLI updates and launches only through registered tool APIs", async () => {
    const user = userEvent.setup();
    settingsApiMock.getToolVersions.mockResolvedValue([
      {
        name: "grok",
        version: "1.0.0",
        latest_version: "1.1.0",
        error: null,
        installed_but_broken: false,
      },
    ]);
    settingsApiMock.getToolLifecycleCapabilities.mockResolvedValue([
      {
        ...capabilities,
        name: "grok",
        can_update: true,
      },
    ]);
    settingsApiMock.runCliLifecycleAction.mockResolvedValue(undefined);
    settingsApiMock.launchToolTerminal.mockResolvedValue(undefined);

    render(<RuntimeLifecycleCard appId="grokbuild" isConfigured />);

    await user.click(
      await screen.findByRole("button", { name: "appLifecycle.update" }),
    );
    await waitFor(() =>
      expect(settingsApiMock.runCliLifecycleAction).toHaveBeenCalledWith(
        "grok",
        "update",
        expect.any(String),
      ),
    );

    await user.click(
      screen.getByRole("button", { name: "appLifecycle.launch" }),
    );
    await waitFor(() =>
      expect(settingsApiMock.launchToolTerminal).toHaveBeenCalledWith("grok"),
    );
  });

  it("launches and restarts DSH through its Web service APIs", async () => {
    const user = userEvent.setup();
    settingsApiMock.getToolVersions.mockResolvedValue([
      {
        name: "dsh",
        version: "1.0.0",
        latest_version: null,
        error: null,
        installed_but_broken: false,
      },
    ]);
    settingsApiMock.getToolLifecycleCapabilities.mockResolvedValue([
      { ...capabilities, name: "dsh" },
    ]);

    render(<RuntimeLifecycleCard appId="dsh" isConfigured />);

    expect(
      await screen.findByText("appLifecycle.dshDescription"),
    ).toBeVisible();
    expect(
      screen.queryByText("appLifecycle.cliDescription"),
    ).not.toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "appLifecycle.dshLaunch" }),
    );

    await waitFor(() =>
      expect(settingsApiMock.launchDsh).toHaveBeenCalledOnce(),
    );
    expect(settingsApiMock.launchToolTerminal).not.toHaveBeenCalled();
    expect(toastMock.success).toHaveBeenCalledWith("appLifecycle.dshStarted");

    await user.click(
      screen.getByRole("button", { name: "appLifecycle.restart" }),
    );

    await waitFor(() =>
      expect(settingsApiMock.restartDsh).toHaveBeenCalledOnce(),
    );
    expect(toastMock.success).toHaveBeenCalledWith("appLifecycle.dshRestarted");
  });

  it("restores the DSH launch button after a launch failure", async () => {
    const user = userEvent.setup();
    settingsApiMock.launchDsh.mockRejectedValueOnce(new Error("port busy"));

    render(<RuntimeLifecycleCard appId="dsh" isConfigured />);

    const launchButton = await screen.findByRole("button", {
      name: "appLifecycle.dshLaunch",
    });
    await user.click(launchButton);

    await waitFor(() => expect(launchButton).toBeEnabled());
    expect(settingsApiMock.launchToolTerminal).not.toHaveBeenCalled();
    expect(toastMock.error).toHaveBeenCalledWith(
      "appLifecycle.dshLaunchFailed",
      { description: "port busy" },
    );
  });

  it("requires confirmation before sending CLI uninstall to the lifecycle API", async () => {
    const user = userEvent.setup();
    settingsApiMock.getToolVersions
      .mockResolvedValueOnce([
        {
          name: "openclaw",
          version: "1.0.0",
          latest_version: null,
          error: null,
          installed_but_broken: false,
        },
      ])
      .mockResolvedValueOnce([
        {
          name: "openclaw",
          version: null,
          latest_version: null,
          error: null,
          installed_but_broken: false,
        },
      ]);
    settingsApiMock.getToolLifecycleCapabilities.mockResolvedValue([
      {
        ...capabilities,
        name: "openclaw",
        can_uninstall: true,
      },
    ]);
    settingsApiMock.runCliLifecycleAction.mockResolvedValue(undefined);

    render(<RuntimeLifecycleCard appId="openclaw" isConfigured />);

    await user.click(
      await screen.findByRole("button", { name: "appLifecycle.uninstall" }),
    );
    expect(settingsApiMock.runCliLifecycleAction).not.toHaveBeenCalled();

    const dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", { name: "appLifecycle.uninstall" }),
    );

    await waitFor(() =>
      expect(settingsApiMock.runCliLifecycleAction).toHaveBeenCalledWith(
        "openclaw",
        "uninstall",
        expect.any(String),
      ),
    );
  });

  it("cancels a restored CLI job by its durable job id", async () => {
    const user = userEvent.setup();
    settingsApiMock.getToolVersions.mockResolvedValue([
      {
        name: "pi",
        version: "1.0.0",
        latest_version: null,
        error: null,
        installed_but_broken: false,
      },
    ]);
    settingsApiMock.getToolLifecycleCapabilities.mockResolvedValue([
      { ...capabilities, name: "pi" },
    ]);
    settingsApiMock.listCliLifecycleJobs.mockResolvedValue([
      cliJob({ id: "durable-pi-job", appId: "pi", state: "running" }),
    ]);

    render(<RuntimeLifecycleCard appId="pi" isConfigured />);

    await user.click(
      await screen.findByRole("button", { name: "appLifecycle.stop" }),
    );

    expect(settingsApiMock.cancelCliLifecycleJob).toHaveBeenCalledWith(
      "durable-pi-job",
    );
  });

  it("sends desktop lifecycle operations through the registered desktop API", async () => {
    const user = userEvent.setup();
    settingsApiMock.getToolVersions.mockResolvedValue([
      {
        name: "claude",
        version: "1.0.0",
        latest_version: null,
        error: null,
        installed_but_broken: false,
      },
    ]);
    settingsApiMock.getToolLifecycleCapabilities.mockResolvedValue([
      { ...capabilities, name: "claude" },
    ]);
    const installedDesktop: DesktopAppStatus = {
      ...desktopStatus,
      id: "claude-desktop",
      display_name: "Claude Desktop",
      installed: true,
      version: "1.0.0",
      latest_version: "1.1.0",
      can_install: false,
      can_update: true,
      can_uninstall: true,
      can_launch: true,
    };
    settingsApiMock.getDesktopAppStatus.mockResolvedValue(installedDesktop);
    settingsApiMock.runDesktopAppLifecycleAction.mockResolvedValue(
      installedDesktop,
    );
    settingsApiMock.launchDesktopApp.mockResolvedValue(undefined);

    render(<RuntimeLifecycleCard appId="claude" isConfigured />);

    await user.click(
      await screen.findByRole("button", { name: "appLifecycle.update" }),
    );
    await waitFor(() =>
      expect(settingsApiMock.runDesktopAppLifecycleAction).toHaveBeenCalledWith(
        "claude-desktop",
        "update",
        expect.any(String),
      ),
    );

    await user.click(
      screen.getByRole("button", { name: "appLifecycle.launchDesktop" }),
    );
    await waitFor(() =>
      expect(settingsApiMock.launchDesktopApp).toHaveBeenCalledWith(
        "claude-desktop",
      ),
    );

    await user.click(
      screen.getByRole("button", { name: "appLifecycle.uninstall" }),
    );
    const dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", { name: "appLifecycle.uninstall" }),
    );
    await waitFor(() =>
      expect(
        settingsApiMock.runDesktopAppLifecycleAction,
      ).toHaveBeenLastCalledWith(
        "claude-desktop",
        "uninstall",
        expect.any(String),
      ),
    );
  });
});
