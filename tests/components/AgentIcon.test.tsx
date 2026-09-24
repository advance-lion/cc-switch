import { act, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import {
  AgentIcon,
  getAgentVisual,
  registerAgentVisual,
} from "@/components/AgentIcon";
import { hasIcon } from "@/icons/extracted";

describe("AgentIcon", () => {
  it("resolves DSH to the monochrome whale instead of the letter fallback", () => {
    const { container } = render(<AgentIcon agentId="dsh" size={20} />);

    expect(hasIcon("dsh")).toBe(true);
    expect(container.querySelector("svg title")?.textContent).toBe(
      "DeepSeek Harness",
    );
    expect(container.querySelector('path[fill="currentColor"]')).not.toBeNull();
    expect(screen.queryByText("D")).not.toBeInTheDocument();
  });

  it("keeps a safe fallback for an Agent without a bundled icon", () => {
    render(<AgentIcon agentId="future-agent" />);

    expect(screen.getByText("FA")).toBeInTheDocument();
  });

  it("renders the bundled Qoder mark for a discovered Agent", () => {
    const { container } = render(<AgentIcon agentId="qoder" size={20} />);

    expect(hasIcon("qoder")).toBe(true);
    expect(container.querySelector("svg title")?.textContent).toBe("Qoder");
    expect(screen.queryByText("Q")).not.toBeInTheDocument();
  });

  it("allows an installed Agent to replace its icon at runtime", () => {
    const { container } = render(<AgentIcon agentId="future-agent" />);
    expect(screen.getByText("FA")).toBeInTheDocument();

    let restore = () => {};
    act(() => {
      restore = registerAgentVisual("future-agent", {
        label: "Future Agent",
        icon: "openai",
      });
    });

    expect(getAgentVisual("future-agent").icon).toBe("openai");
    expect(container.querySelector("svg title")?.textContent).toBe("OpenAI");
    expect(screen.queryByText("FA")).not.toBeInTheDocument();

    act(restore);
  });
});
