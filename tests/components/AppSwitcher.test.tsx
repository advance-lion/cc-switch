import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AppSwitcher } from "@/components/AppSwitcher";
import { DEFAULT_VISIBLE_APPS } from "@/config/appConfig";
import type { ManagedAgent } from "@/lib/managedAgents";

const qoder: ManagedAgent = {
  id: "qoder",
  tool: "qoder",
  name: "Qoder",
  shortLabel: "Qoder",
  icon: "qoder",
  packageName: "@qoder-ai/qodercli",
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
        managedAgents={[qoder]}
        onSwitchManagedAgent={onSwitchManagedAgent}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "Qoder" }));
    expect(onSwitchManagedAgent).toHaveBeenCalledWith("qoder");
    expect(localStorage.getItem("cc-switch-last-app")).toBe("managed:qoder");
  });
});
