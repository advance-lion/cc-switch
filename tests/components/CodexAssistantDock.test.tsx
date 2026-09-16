import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CodexAssistantDock } from "@/components/assistant/CodexAssistantDock";
import type { CodexAssistantEvent } from "@/lib/api";

const settingsApiMock = vi.hoisted(() => ({
  getToolVersions: vi.fn(),
  runToolLifecycleAction: vi.fn(),
  startCodexAssistantSession: vi.fn(),
  sendCodexAssistantMessage: vi.fn(),
  respondCodexAssistantApproval: vi.fn(),
  cancelCodexAssistantRun: vi.fn(),
  closeCodexAssistantSession: vi.fn(),
  onCodexAssistantEvent: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("@/lib/api", () => ({
  isCodexAssistantWebBridgeActive: () => false,
  settingsApi: settingsApiMock,
}));

const renderDock = (providerReady = true, onInstallationCompleted = vi.fn()) =>
  render(
    <CodexAssistantDock
      providerReady={providerReady}
      onOpenCodexConfiguration={vi.fn()}
      onInstallationCompleted={onInstallationCompleted}
    />,
  );

describe("CodexAssistantDock", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    settingsApiMock.getToolVersions.mockResolvedValue([
      { name: "codex", version: "0.50.0", installed_but_broken: false },
    ]);
    settingsApiMock.startCodexAssistantSession.mockResolvedValue("session-1");
    settingsApiMock.sendCodexAssistantMessage.mockResolvedValue(undefined);
    settingsApiMock.respondCodexAssistantApproval.mockResolvedValue(undefined);
    settingsApiMock.cancelCodexAssistantRun.mockResolvedValue(true);
    settingsApiMock.closeCodexAssistantSession.mockResolvedValue(true);
    settingsApiMock.onCodexAssistantEvent.mockResolvedValue(() => {});
  });

  const emitAssistantEvent = async (event: CodexAssistantEvent) => {
    await waitFor(() =>
      expect(settingsApiMock.onCodexAssistantEvent).toHaveBeenCalled(),
    );
    const listener = settingsApiMock.onCodexAssistantEvent.mock.calls[0]?.[0];
    expect(listener).toBeTypeOf("function");
    await act(async () => {
      listener(event);
    });
  };

  const openDock = async () => {
    await userEvent.click(
      await screen.findByRole("button", { name: "codexAssistant.open" }),
    );
    await screen.findByText("codexAssistant.title");
  };

  const sendMessage = async (message: string) => {
    const input = await screen.findByPlaceholderText(
      "codexAssistant.chatPlaceholder",
    );
    await userEvent.type(input, message);
    await userEvent.click(
      screen.getByRole("button", { name: "codexAssistant.sendChat" }),
    );
  };

  it("installs Codex through the fixed lifecycle action and refreshes the app", async () => {
    settingsApiMock.getToolVersions
      .mockResolvedValueOnce([
        { name: "codex", version: null, installed_but_broken: false },
      ])
      .mockResolvedValueOnce([
        { name: "codex", version: "0.51.0", installed_but_broken: false },
      ]);
    settingsApiMock.runToolLifecycleAction.mockResolvedValue(undefined);
    const onInstallationCompleted = vi.fn();

    renderDock(true, onInstallationCompleted);
    await openDock();

    await userEvent.click(
      screen.getByRole("button", {
        name: "codexAssistant.codexCli codexAssistant.installCli",
      }),
    );

    await waitFor(() =>
      expect(settingsApiMock.runToolLifecycleAction).toHaveBeenCalledWith(
        ["codex"],
        "install",
      ),
    );
    await waitFor(() =>
      expect(onInstallationCompleted).toHaveBeenCalledWith("codex"),
    );
    expect(settingsApiMock.getToolVersions).toHaveBeenCalledTimes(2);
  });
  it("opens as a normal conversation without plan controls", async () => {
    renderDock();
    await openDock();

    expect(
      screen.getByPlaceholderText("codexAssistant.chatPlaceholder"),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "codexAssistant.modePlan" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("codexAssistant.installLocation")).toBeNull();
  });

  it("requires Codex CLI before a message can be sent", async () => {
    settingsApiMock.getToolVersions.mockResolvedValue([
      { name: "codex", version: null, installed_but_broken: false },
    ]);

    renderDock();
    await openDock();

    expect(
      await screen.findByText("codexAssistant.cliRequired"),
    ).toBeInTheDocument();
    expect(settingsApiMock.startCodexAssistantSession).not.toHaveBeenCalled();
  });

  it("requires a configured provider before a message can be sent", async () => {
    renderDock(false);
    await openDock();

    expect(
      await screen.findByText("codexAssistant.providerRequired"),
    ).toBeInTheDocument();
    expect(settingsApiMock.startCodexAssistantSession).not.toHaveBeenCalled();
  });

  it("starts one session and sends subsequent messages through it", async () => {
    renderDock();
    await openDock();
    await sendMessage("你好");

    await waitFor(() =>
      expect(settingsApiMock.sendCodexAssistantMessage).toHaveBeenCalledWith(
        "session-1",
        "你好",
      ),
    );
    expect(settingsApiMock.startCodexAssistantSession).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("你好")).toBeInTheDocument();

    await emitAssistantEvent({
      sessionId: "session-1",
      kind: "message",
      message: "你好！",
    });
    await emitAssistantEvent({
      sessionId: "session-1",
      kind: "message",
      message: "有什么可以帮你？",
    });
    await emitAssistantEvent({
      sessionId: "session-1",
      kind: "finished",
      success: true,
      cancelled: false,
    });
    expect(
      await screen.findByText("你好！有什么可以帮你？"),
    ).toBeInTheDocument();

    await sendMessage("第二个问题");
    await waitFor(() =>
      expect(
        settingsApiMock.sendCodexAssistantMessage,
      ).toHaveBeenLastCalledWith("session-1", "第二个问题"),
    );
    expect(settingsApiMock.startCodexAssistantSession).toHaveBeenCalledTimes(1);
  });

  it("shows command approvals and sends the selected decision", async () => {
    renderDock();
    await openDock();
    await sendMessage("检查项目");
    await waitFor(() =>
      expect(settingsApiMock.sendCodexAssistantMessage).toHaveBeenCalled(),
    );

    await emitAssistantEvent({
      sessionId: "session-1",
      kind: "approval",
      approval: {
        id: "approval-command",
        type: "command",
        command: "pnpm test:unit",
        cwd: "C:\\workspace\\project",
        reason: "Run the focused test suite",
        networkHost: "registry.npmjs.org",
        grantRoot: null,
        allowForSession: true,
        availableDecisions: ["accept", "acceptForSession", "decline", "cancel"],
      },
    });

    expect(screen.getByText("pnpm test:unit")).toBeInTheDocument();
    expect(screen.getByText("C:\\workspace\\project")).toBeInTheDocument();
    expect(screen.getByText("registry.npmjs.org")).toBeInTheDocument();

    await userEvent.click(
      screen.getByRole("button", { name: "Allow for session" }),
    );
    await waitFor(() =>
      expect(
        settingsApiMock.respondCodexAssistantApproval,
      ).toHaveBeenCalledWith(
        "session-1",
        "approval-command",
        "acceptForSession",
      ),
    );
    expect(
      screen.queryByTestId("codex-approval-approval-command"),
    ).not.toBeInTheDocument();
  });

  it("only renders approval decisions advertised by app-server", async () => {
    renderDock();
    await openDock();
    await sendMessage("执行受控命令");
    await waitFor(() =>
      expect(settingsApiMock.sendCodexAssistantMessage).toHaveBeenCalled(),
    );

    await emitAssistantEvent({
      sessionId: "session-1",
      kind: "approval",
      approval: {
        id: "approval-limited",
        type: "command",
        command: "tool --check",
        cwd: "C:\\workspace",
        reason: "Check the local tool",
        networkHost: null,
        grantRoot: null,
        allowForSession: false,
        availableDecisions: ["accept", "cancel"],
      },
    });

    expect(
      screen.getByRole("button", { name: "Allow once" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Cancel task" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Deny" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Allow for session" }),
    ).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Cancel task" }));
    await waitFor(() =>
      expect(
        settingsApiMock.respondCodexAssistantApproval,
      ).toHaveBeenCalledWith("session-1", "approval-limited", "cancel"),
    );
  });

  it("shows file approvals without a session-wide option and can deny them", async () => {
    renderDock();
    await openDock();
    await sendMessage("修改配置");
    await waitFor(() =>
      expect(settingsApiMock.sendCodexAssistantMessage).toHaveBeenCalled(),
    );

    await emitAssistantEvent({
      sessionId: "session-1",
      kind: "approval",
      approval: {
        id: "approval-file",
        type: "fileChange",
        command: null,
        cwd: null,
        reason: "Update the provider configuration",
        networkHost: null,
        grantRoot: "C:\\workspace\\project",
        allowForSession: false,
        availableDecisions: ["accept", "decline"],
      },
    });

    expect(
      screen.getByText("Update the provider configuration"),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Allow for session" }),
    ).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Deny" }));
    await waitFor(() =>
      expect(
        settingsApiMock.respondCodexAssistantApproval,
      ).toHaveBeenCalledWith("session-1", "approval-file", "decline"),
    );
  });

  it("prefills an install intent as a conversation message", async () => {
    render(
      <CodexAssistantDock
        providerReady={true}
        onOpenCodexConfiguration={vi.fn()}
        installIntent={{ tool: "gemini", appName: "Gemini CLI" }}
        openRequestId={1}
      />,
    );

    expect(
      await screen.findByDisplayValue("codexAssistant.installRequest"),
    ).toBeInTheDocument();
    expect(screen.queryByText("codexAssistant.installLocation")).toBeNull();
    expect(
      screen.queryByRole("button", { name: "codexAssistant.modePlan" }),
    ).not.toBeInTheDocument();
  });

  it("clears a disconnected session and reconnects on the next message", async () => {
    settingsApiMock.startCodexAssistantSession
      .mockResolvedValueOnce("session-1")
      .mockResolvedValueOnce("session-2");
    renderDock();
    await openDock();
    await sendMessage("first");
    await waitFor(() =>
      expect(settingsApiMock.sendCodexAssistantMessage).toHaveBeenCalledWith(
        "session-1",
        "first",
      ),
    );

    await emitAssistantEvent({
      sessionId: "session-1",
      kind: "disconnected",
      message: "Codex app-server disconnected",
    });
    expect(
      await screen.findByText("Codex app-server disconnected"),
    ).toBeInTheDocument();

    await sendMessage("second");
    await waitFor(() =>
      expect(settingsApiMock.sendCodexAssistantMessage).toHaveBeenLastCalledWith(
        "session-2",
        "second",
      ),
    );
    expect(settingsApiMock.startCodexAssistantSession).toHaveBeenCalledTimes(2);
  });

  it("closes the app-server session when the dock closes", async () => {
    renderDock();
    await openDock();
    await sendMessage("你好");
    await waitFor(() =>
      expect(settingsApiMock.sendCodexAssistantMessage).toHaveBeenCalled(),
    );

    await userEvent.click(screen.getByRole("button", { name: "common.close" }));
    expect(settingsApiMock.closeCodexAssistantSession).toHaveBeenCalledWith(
      "session-1",
    );
  });
});
