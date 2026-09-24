import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AppSwitcher } from "@/components/AppSwitcher";
import { DEFAULT_VISIBLE_APPS } from "@/config/appConfig";
import type { ManagedAgent } from "@/lib/managedAgents";

const customAgent: ManagedAgent = {
  id: "custom-agent",
  tool: "custom-agent",
  name: "Custom Agent",
  shortLabel: "Custom Agent",
  icon: "custom-agent",
  packageName: "@example/custom-agent",
  version: "1.1.62",
  installedButBroken: false,
  error: null,
  providerIntegration: "planned",
};

describe("AppSwitcher managed Agents", () => {
  it("shows and selects an auto-detected Agent", async () => {
    const onSwitchManagedAgent = vi.fn();
    render(
      <AppSwitcher
        activeApp="claude"
        onSwitch={vi.fn()}
        visibleApps={DEFAULT_VISIBLE_APPS}
        managedAgents={[customAgent]}
        onSwitchManagedAgent={onSwitchManagedAgent}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "Custom Agent" }));
    expect(onSwitchManagedAgent).toHaveBeenCalledWith("custom-agent");
    expect(localStorage.getItem("cc-switch-last-app")).toBe(
      "managed:custom-agent",
    );
  });
});
