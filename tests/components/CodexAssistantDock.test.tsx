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

  it("requests a managed-Agent rescan after a successful assistant turn", async () => {
    const onRunCompleted = vi.fn();
    render(
      <CodexAssistantDock
        providerReady={true}
        onOpenCodexConfiguration={vi.fn()}
        onRunCompleted={onRunCompleted}
      />,
    );
    await openDock();
    await sendMessage("安装 Qoder");
    await waitFor(() =>
      expect(settingsApiMock.sendCodexAssistantMessage).toHaveBeenCalled(),
    );

    await emitAssistantEvent({
      sessionId: "session-1",
      kind: "finished",
      success: true,
      cancelled: false,
    });

    expect(onRunCompleted).toHaveBeenCalledTimes(1);
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
      expect(
        settingsApiMock.sendCodexAssistantMessage,
      ).toHaveBeenLastCalledWith("session-2", "second"),
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
