import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CodexAssistantDock } from "@/components/assistant/CodexAssistantDock";
import type { CodexAssistantEvent } from "@/lib/api";

const settingsApiMock = vi.hoisted(() => ({
  getToolVersions: vi.fn(),
  runToolLifecycleAction: vi.fn(),
  pickDirectory: vi.fn(),
  startCodexAssistantPlan: vi.fn(),
  startCodexAssistantChat: vi.fn(),
  executeCodexAssistantPlan: vi.fn(),
  cancelCodexAssistantRun: vi.fn(),
  onCodexAssistantEvent: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("@/lib/api", () => ({
  isCodexAssistantWebBridgeActive: () => false,
  settingsApi: settingsApiMock,
}));

const renderDock = (providerReady = true) =>
  render(
    <CodexAssistantDock
      providerReady={providerReady}
      onOpenCodexConfiguration={vi.fn()}
    />,
  );

describe("CodexAssistantDock", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    settingsApiMock.getToolVersions.mockResolvedValue([
      { name: "codex", version: "0.50.0", installed_but_broken: false },
    ]);
    settingsApiMock.startCodexAssistantChat.mockResolvedValue("run-chat-1");
    settingsApiMock.startCodexAssistantPlan.mockResolvedValue("run-plan-1");
  });

  const emitAssistantEvent = async (event: CodexAssistantEvent) => {
    const listener = settingsApiMock.onCodexAssistantEvent.mock.calls[0]?.[0];
    expect(listener).toBeTypeOf("function");
    await act(async () => {
      listener(event);
    });
  };

  it("opens the controlled install panel from the floating entry", async () => {
    renderDock();

    await userEvent.click(
      await screen.findByRole("button", { name: "codexAssistant.open" }),
    );

    expect(await screen.findByText("codexAssistant.title")).toBeInTheDocument();
    // The dock is an install planner: it offers a plan step, never a shell.
    expect(screen.queryByRole("textbox", { name: /shell/i })).toBeNull();
  });

  it("requires Codex CLI before any plan can be created", async () => {
    settingsApiMock.getToolVersions.mockResolvedValue([
      { name: "codex", version: null, installed_but_broken: false },
    ]);

    renderDock();
    await userEvent.click(
      await screen.findByRole("button", { name: "codexAssistant.open" }),
    );

    expect(
      await screen.findByText("codexAssistant.cliRequired"),
    ).toBeInTheDocument();
    expect(settingsApiMock.startCodexAssistantPlan).not.toHaveBeenCalled();
  });

  it("requires a configured provider before any plan can be created", async () => {
    renderDock(false);
    await userEvent.click(
      await screen.findByRole("button", { name: "codexAssistant.open" }),
    );

    expect(
      await screen.findByText("codexAssistant.providerRequired"),
    ).toBeInTheDocument();
    expect(settingsApiMock.startCodexAssistantPlan).not.toHaveBeenCalled();
  });

  it("sends chat messages without requiring an install directory", async () => {
    renderDock();
    await userEvent.click(
      await screen.findByRole("button", { name: "codexAssistant.open" }),
    );

    const input = await screen.findByPlaceholderText(
      "codexAssistant.chatPlaceholder",
    );
    await userEvent.type(input, "你好");
    await userEvent.click(
      screen.getByRole("button", { name: "codexAssistant.sendChat" }),
    );

    await waitFor(() =>
      expect(settingsApiMock.startCodexAssistantChat).toHaveBeenCalledWith(
        "你好",
        [],
      ),
    );
    expect(settingsApiMock.startCodexAssistantPlan).not.toHaveBeenCalled();
    // The user's own message appears immediately as a chat bubble.
    expect(await screen.findByText("你好")).toBeInTheDocument();
  });

  it("renders the assistant answer and sends prior turns as history", async () => {
    renderDock();
    await userEvent.click(
      await screen.findByRole("button", { name: "codexAssistant.open" }),
    );

    const input = await screen.findByPlaceholderText(
      "codexAssistant.chatPlaceholder",
    );
    await userEvent.type(input, "你好");
    await userEvent.click(
      screen.getByRole("button", { name: "codexAssistant.sendChat" }),
    );
    await waitFor(() =>
      expect(settingsApiMock.startCodexAssistantChat).toHaveBeenCalled(),
    );

    await emitAssistantEvent({
      runId: "run-chat-1",
      kind: "message",
      message: "你好！有什么可以帮你？",
    });
    await emitAssistantEvent({
      runId: "run-chat-1",
      kind: "finished",
      success: true,
    });

    expect(
      await screen.findByText("你好！有什么可以帮你？"),
    ).toBeInTheDocument();

    await userEvent.type(input, "第二个问题");
    await userEvent.click(
      screen.getByRole("button", { name: "codexAssistant.sendChat" }),
    );
    await waitFor(() =>
      expect(settingsApiMock.startCodexAssistantChat).toHaveBeenLastCalledWith(
        "第二个问题",
        [
          { role: "user", content: "你好" },
          { role: "assistant", content: "你好！有什么可以帮你？" },
        ],
      ),
    );
  });

  it("switches to the install plan mode explicitly", async () => {
    renderDock();
    await userEvent.click(
      await screen.findByRole("button", { name: "codexAssistant.open" }),
    );

    await userEvent.click(
      await screen.findByRole("button", { name: "codexAssistant.modePlan" }),
    );

    expect(
      await screen.findByText("codexAssistant.installLocation"),
    ).toBeInTheDocument();
    expect(
      screen.getByPlaceholderText("codexAssistant.requestPlaceholder"),
    ).toBeInTheDocument();
  });

  it("opens in plan mode when launched with an install intent", async () => {
    render(
      <CodexAssistantDock
        providerReady={true}
        onOpenCodexConfiguration={vi.fn()}
        installIntent={{ tool: "gemini", appName: "Gemini CLI" }}
        openRequestId={1}
      />,
    );

    expect(
      await screen.findByText("codexAssistant.installLocation"),
    ).toBeInTheDocument();
    // Chat-only UI stays hidden in plan mode.
    expect(
      screen.queryByText("codexAssistant.chatEmpty"),
    ).not.toBeInTheDocument();
  });
});
