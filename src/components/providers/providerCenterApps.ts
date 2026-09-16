import type { ProviderCenterApp } from "@/lib/api/providerCenter";

export const PROVIDER_CENTER_APPS: ProviderCenterApp[] = [
  "claude",
  "claude-desktop",
  "codex",
  "gemini",
  "grokbuild",
  "opencode",
  "openclaw",
  "hermes",
  "pi",
];

export const PROVIDER_CENTER_APP_LABELS: Record<ProviderCenterApp, string> = {
  claude: "Claude Code",
  "claude-desktop": "Claude Desktop",
  codex: "Codex",
  gemini: "Gemini CLI",
  grokbuild: "Grok Build",
  opencode: "OpenCode",
  openclaw: "OpenClaw",
  hermes: "Hermes",
  pi: "Pi",
};

export function providerCenterAppLabel(appType: string): string {
  return (
    PROVIDER_CENTER_APP_LABELS[appType as ProviderCenterApp] ?? appType
  );
}
