import path from "node:path";
import { randomUUID } from "node:crypto";
import { spawn } from "node:child_process";
import { promises as fs } from "node:fs";
import os from "node:os";
import type { IncomingMessage, ServerResponse } from "node:http";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { codeInspectorPlugin } from "code-inspector-plugin";

const WEB_ASSISTANT_BASE = "/__cc-switch-dev/codex-assistant";
const MAX_REQUEST_LENGTH = 6_000;

type PreviewAssistantSession = {
  id: string;
  history: ChatTurn[];
};

type DesktopAppId = "codex-desktop" | "claude-desktop";

type DesktopAppStatus = {
  id: DesktopAppId;
  display_name: string;
  installed: boolean;
  version: string | null;
  latest_version: string | null;
  path: string | null;
  package_identity: string | null;
  installation_source: string;
  can_install: boolean;
  can_update: boolean;
  can_uninstall: boolean;
  can_launch: boolean;
  reason: string | null;
};

type DesktopAppManifest = {
  id: DesktopAppId;
  displayName: string;
  windowsPackageName: string;
  windowsAppIdSuffix: string;
  wingetId: string;
  wingetSource: string;
  installationSource: "microsoft_store" | "official_appx";
};

type DesktopAppxRecord = {
  version: string;
  package_full_name: string;
  package_family_name: string;
  install_location: string;
};

const DESKTOP_APPS: Record<DesktopAppId, DesktopAppManifest> = {
  "codex-desktop": {
    id: "codex-desktop",
    displayName: "Codex Desktop",
    windowsPackageName: "OpenAI.Codex",
    windowsAppIdSuffix: "App",
    wingetId: "9PLM9XGG6VKS",
    wingetSource: "msstore",
    installationSource: "microsoft_store",
  },
  "claude-desktop": {
    id: "claude-desktop",
    displayName: "Claude Desktop",
    windowsPackageName: "Claude",
    windowsAppIdSuffix: "Claude",
    wingetId: "Anthropic.Claude",
    wingetSource: "winget",
    installationSource: "official_appx",
  },
};

function webAssistantBridge() {
  const sessions = new Map<string, PreviewAssistantSession>();
  const listeners = new Set<ServerResponse>();
  const recentEvents: Array<Record<string, unknown>> = [];

  const emit = (payload: Record<string, unknown>) => {
    recentEvents.push(payload);
    if (recentEvents.length > 200)
      recentEvents.splice(0, recentEvents.length - 200);
    const event = `data: ${JSON.stringify(payload)}\n\n`;
    for (const response of listeners) response.write(event);
  };

  const sendJson = (
    response: ServerResponse,
    status: number,
    payload: unknown,
  ) => {
    response.writeHead(status, { "Content-Type": "application/json" });
    response.end(JSON.stringify(payload));
  };

  const readJson = async (
    request: IncomingMessage,
  ): Promise<Record<string, unknown>> => {
    let body = "";
    for await (const chunk of request) {
      body += chunk;
      if (body.length > 10_000) throw new Error("请求过长");
    }
    return JSON.parse(body || "{}") as Record<string, unknown>;
  };

  const desktopManifest = (value: unknown): DesktopAppManifest => {
    if (value !== "codex-desktop" && value !== "claude-desktop") {
      throw new Error(
        `Unsupported desktop application: ${String(value ?? "")}`,
      );
    }
    return DESKTOP_APPS[value];
  };

  const commandOutput = async (
    executable: string,
    args: string[],
    label: string,
  ): Promise<string> =>
    await new Promise((resolve, reject) => {
      const child = spawn(executable, args, {
        stdio: ["ignore", "pipe", "pipe"],
        windowsHide: true,
      });
      let stdout = "";
      let stderr = "";
      let settled = false;
      child.stdout.on("data", (chunk) => (stdout += chunk));
      child.stderr.on("data", (chunk) => (stderr += chunk));
      child.on("error", (error) => {
        if (settled) return;
        settled = true;
        reject(new Error(`Failed to start ${label}: ${error.message}`));
      });
      child.on("close", (code) => {
        if (settled) return;
        settled = true;
        const output = stdout.trim();
        const errorOutput = stderr.trim();
        if (code !== 0) {
          reject(
            new Error(
              `${label} failed with exit code ${code ?? -1}${
                errorOutput || output ? `: ${errorOutput || output}` : ""
              }`,
            ),
          );
          return;
        }
        resolve(output);
      });
    });

  const powershellOutput = async (script: string, label: string) =>
    await commandOutput(
      "powershell.exe",
      [
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
      ],
      label,
    );

  const unsupportedDesktopStatus = (
    manifest: DesktopAppManifest,
  ): DesktopAppStatus => ({
    id: manifest.id,
    display_name: manifest.displayName,
    installed: false,
    version: null,
    latest_version: null,
    path: null,
    package_identity: null,
    installation_source: "unsupported_platform",
    can_install: false,
    can_update: false,
    can_uninstall: false,
    can_launch: false,
    reason:
      "Desktop application management in the web development bridge is supported on Windows only",
  });

  const readDesktopAppxRecord = async (
    manifest: DesktopAppManifest,
  ): Promise<DesktopAppxRecord | null> => {
    if (process.platform !== "win32") return null;
    const script = `[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$pkg = Get-AppxPackage -Name '${manifest.windowsPackageName}' | Sort-Object Version -Descending | Select-Object -First 1
if ($null -eq $pkg) { Write-Output 'null'; exit 0 }
[pscustomobject]@{
  version = $pkg.Version.ToString()
  package_full_name = $pkg.PackageFullName
  package_family_name = $pkg.PackageFamilyName
  install_location = $pkg.InstallLocation
} | ConvertTo-Json -Compress`;
    const output = await powershellOutput(
      script,
      "desktop application detection",
    );
    if (!output || output === "null") return null;
    const record = JSON.parse(output) as Partial<DesktopAppxRecord>;
    if (
      typeof record.version !== "string" ||
      typeof record.package_full_name !== "string" ||
      typeof record.package_family_name !== "string" ||
      typeof record.install_location !== "string"
    ) {
      throw new Error("Invalid desktop package metadata");
    }
    if (
      !record.package_full_name.startsWith(manifest.windowsPackageName) ||
      !record.package_family_name.startsWith(manifest.windowsPackageName)
    ) {
      throw new Error(
        `Detected package identity does not match ${manifest.displayName}`,
      );
    }
    return record as DesktopAppxRecord;
  };

  const detectDesktopApp = async (
    manifest: DesktopAppManifest,
  ): Promise<DesktopAppStatus> => {
    if (process.platform !== "win32") {
      return unsupportedDesktopStatus(manifest);
    }
    const record = await readDesktopAppxRecord(manifest);
    if (!record) {
      return {
        id: manifest.id,
        display_name: manifest.displayName,
        installed: false,
        version: null,
        latest_version: null,
        path: null,
        package_identity: null,
        installation_source: "not_installed",
        can_install: true,
        can_update: false,
        can_uninstall: false,
        can_launch: false,
        reason: null,
      };
    }
    return {
      id: manifest.id,
      display_name: manifest.displayName,
      installed: true,
      version: record.version,
      latest_version: null,
      path: record.install_location,
      package_identity: record.package_full_name,
      installation_source: manifest.installationSource,
      can_install: false,
      can_update: true,
      can_uninstall: true,
      can_launch: true,
      reason: null,
    };
  };

  const latestDesktopVersion = async (manifest: DesktopAppManifest) => {
    if (process.platform !== "win32") {
      throw new Error(
        `Automatic ${manifest.displayName} update checks are currently supported on Windows only`,
      );
    }
    const output = await commandOutput(
      "winget.exe",
      [
        "show",
        "--id",
        manifest.wingetId,
        "--exact",
        "--source",
        manifest.wingetSource,
        "--accept-source-agreements",
        "--disable-interactivity",
      ],
      "desktop update lookup",
    );
    for (const line of output.split(/\r?\n/)) {
      const match = line.trim().match(/^(?:Version|版本)\s*[:：]\s*(.+)$/i);
      if (match?.[1]?.trim()) return match[1].trim();
    }
    throw new Error(
      `Could not determine the latest ${manifest.displayName} version from winget`,
    );
  };

  const checkDesktopAppUpdates = async (
    manifest: DesktopAppManifest,
  ): Promise<DesktopAppStatus> => {
    const status = await detectDesktopApp(manifest);
    const latest = await latestDesktopVersion(manifest);
    return { ...status, latest_version: latest };
  };

  const launchDesktopApp = async (manifest: DesktopAppManifest) => {
    if (process.platform !== "win32") {
      throw new Error(
        "Desktop application launch in the web development bridge is supported on Windows only",
      );
    }
    const record = await readDesktopAppxRecord(manifest);
    if (!record) throw new Error(`${manifest.displayName} is not installed`);
    const child = spawn(
      "explorer.exe",
      [
        `shell:AppsFolder\\${record.package_family_name}!${manifest.windowsAppIdSuffix}`,
      ],
      { detached: true, stdio: "ignore", windowsHide: true },
    );
    child.unref();
  };

  const MAX_CHAT_HISTORY_TURNS = 12;
  const MAX_CHAT_TURN_LENGTH = 2_000;
  const MAX_CHAT_HISTORY_CHARS = 12_000;

  type ChatTurn = { role: "user" | "assistant"; content: string };

  // Mirrors normalize_chat_history in src-tauri/src/commands/codex_assistant.rs.
  const normalizeChatHistory = (history: unknown): ChatTurn[] => {
    if (history === undefined || history === null) return [];
    if (!Array.isArray(history)) throw new Error("对话历史格式不正确");
    let normalized: ChatTurn[] = [];
    for (const entry of history) {
      if (!entry || typeof entry !== "object")
        throw new Error("对话历史格式不正确");
      const { role, content } = entry as { role?: unknown; content?: unknown };
      if (role !== "user" && role !== "assistant")
        throw new Error("对话历史包含非法角色");
      if (typeof content !== "string") throw new Error("对话历史格式不正确");
      const trimmed = content.trim();
      if (!trimmed) continue;
      normalized.push({
        role,
        content: Array.from(trimmed).slice(0, MAX_CHAT_TURN_LENGTH).join(""),
      });
    }
    if (normalized.length > MAX_CHAT_HISTORY_TURNS) {
      normalized = normalized.slice(-MAX_CHAT_HISTORY_TURNS);
    }
    while (
      normalized.reduce((total, turn) => total + turn.content.length, 0) >
      MAX_CHAT_HISTORY_CHARS
    ) {
      normalized.shift();
    }
    return normalized;
  };

  const getSession = (value: unknown) => {
    if (typeof value !== "string" || !value.trim()) return undefined;
    return sessions.get(value.trim());
  };

  const createSession = (history: unknown) => {
    const session: PreviewAssistantSession = {
      id: randomUUID(),
      history: normalizeChatHistory(history),
    };
    sessions.set(session.id, session);
    return session;
  };

  const resolveSession = (body: Record<string, unknown>) =>
    getSession(body.sessionId ?? body.session_id) ??
    createSession(body.history);

  const probeToolVersion = async (
    executable: string,
    cwd?: string,
    env?: NodeJS.ProcessEnv,
  ) => {
    return await new Promise<{ success: boolean; output: string }>(
      (resolve) => {
        const child = spawn(executable, ["--version"], {
          cwd,
          stdio: ["ignore", "pipe", "pipe"],
          windowsHide: true,
          env,
        });
        let text = "";
        child.stdout.on("data", (chunk) => (text += chunk));
        child.stderr.on("data", (chunk) => (text += chunk));
        child.on("error", () => resolve({ success: false, output: "" }));
        child.on("close", (code) =>
          resolve({
            success: code === 0 && Boolean(text.trim()),
            output: text.trim(),
          }),
        );
      },
    );
  };

  return {
    name: "cc-switch-web-codex-assistant",
    configureServer(server: {
      middlewares: {
        use: (
          handler: (
            request: IncomingMessage,
            response: ServerResponse,
            next: () => void,
          ) => void,
        ) => void;
      };
    }) {
      server.middlewares.use((request, response, next) => {
        if (!request.url?.startsWith(WEB_ASSISTANT_BASE)) return next();
        const requestUrl = new URL(request.url, "http://127.0.0.1");
        const route = requestUrl.pathname;
        if (
          request.method === "GET" &&
          route === `${WEB_ASSISTANT_BASE}/events`
        ) {
          response.writeHead(200, {
            "Content-Type": "text/event-stream",
            "Cache-Control": "no-cache",
            Connection: "keep-alive",
          });
          response.write("retry: 1000\n\n");
          listeners.add(response);
          request.on("close", () => listeners.delete(response));
          return;
        }
        if (
          request.method === "GET" &&
          route === `${WEB_ASSISTANT_BASE}/recent-events`
        ) {
          return sendJson(response, 200, { events: recentEvents });
        }
        if (
          request.method === "GET" &&
          route === `${WEB_ASSISTANT_BASE}/status`
        ) {
          void (async () => {
            const result = await probeToolVersion("codex");
            sendJson(response, 200, {
              available: result.success,
              version: result.output || null,
            });
          })();
          return;
        }
        if (
          request.method === "GET" &&
          route === `${WEB_ASSISTANT_BASE}/desktop-status`
        ) {
          void detectDesktopApp(DESKTOP_APPS["codex-desktop"])
            .then((status) =>
              sendJson(response, 200, {
                installed: status.installed,
                version: status.version,
                path: status.path,
              }),
            )
            .catch((error) =>
              sendJson(response, 400, {
                error: error instanceof Error ? error.message : String(error),
              }),
            );
          return;
        }
        if (
          request.method === "GET" &&
          route === `${WEB_ASSISTANT_BASE}/desktop-app-status`
        ) {
          void Promise.resolve()
            .then(() =>
              detectDesktopApp(
                desktopManifest(requestUrl.searchParams.get("app")),
              ),
            )
            .then((status) => sendJson(response, 200, status))
            .catch((error) =>
              sendJson(response, 400, {
                error: error instanceof Error ? error.message : String(error),
              }),
            );
          return;
        }
        if (request.method !== "POST")
          return sendJson(response, 405, { error: "Method not allowed" });
        void readJson(request)
          .then(async (body) => {
            if (
              route === `${WEB_ASSISTANT_BASE}/session` ||
              route === `${WEB_ASSISTANT_BASE}/sessions`
            ) {
              const session = createSession(body.history);
              emit({ sessionId: session.id, kind: "session", status: "ready" });
              return sendJson(response, 201, { sessionId: session.id });
            }
            if (
              route === `${WEB_ASSISTANT_BASE}/chat` ||
              route === `${WEB_ASSISTANT_BASE}/message` ||
              route === `${WEB_ASSISTANT_BASE}/messages`
            ) {
              const requestText =
                typeof body.request === "string"
                  ? body.request
                  : typeof body.message === "string"
                    ? body.message
                    : typeof body.content === "string"
                      ? body.content
                      : "";
              if (!requestText.trim()) throw new Error("请输入想问的内容");
              if (requestText.length > MAX_REQUEST_LENGTH)
                throw new Error("请求过长");

              const session = resolveSession(body);
              const requestContent = requestText.trim();
              const suppliedHistory =
                body.history === undefined
                  ? session.history
                  : normalizeChatHistory(body.history);
              const history = normalizeChatHistory(suppliedHistory);

              const answer =
                "这是浏览器开发预览中的模拟回答。我可以在同一个对话里帮助你了解 AI Agent、安装方式、Provider 和配置流程；浏览器预览不会执行真实命令或修改系统。";
              session.history = normalizeChatHistory([
                ...history,
                { role: "user", content: requestContent },
                { role: "assistant", content: answer },
              ]);
              emit({ sessionId: session.id, kind: "started" });
              emit({
                sessionId: session.id,
                kind: "message",
                message: answer,
              });
              emit({
                sessionId: session.id,
                kind: "finished",
                success: true,
                cancelled: false,
                message: answer,
              });
              return sendJson(response, 202, {
                accepted: true,
                sessionId: session.id,
              });
            }
            if (route === `${WEB_ASSISTANT_BASE}/cancel`) {
              const session = getSession(
                body.sessionId ?? body.session_id ?? body.runId,
              );
              if (!session) {
                throw new Error("没有正在运行的 Codex 任务");
              }
              emit({
                sessionId: session.id,
                kind: "finished",
                success: false,
                cancelled: true,
                message: "已取消任务。浏览器预览未执行任何更改。",
              });
              return sendJson(response, 200, { cancelled: true });
            }
            if (route === `${WEB_ASSISTANT_BASE}/close`) {
              const sessionId = body.sessionId ?? body.session_id;
              if (typeof sessionId !== "string")
                throw new Error("Codex 对话不存在或已失效");
              const closed = sessions.delete(sessionId);
              return sendJson(response, 200, { closed });
            }
            if (route === `${WEB_ASSISTANT_BASE}/desktop-app-check-updates`) {
              const status = await checkDesktopAppUpdates(
                desktopManifest(body.app),
              );
              return sendJson(response, 200, status);
            }
            if (route === `${WEB_ASSISTANT_BASE}/desktop-app-action`) {
              const manifest = desktopManifest(body.app);
              if (
                body.action !== "install" &&
                body.action !== "update" &&
                body.action !== "uninstall"
              ) {
                throw new Error(
                  `Unsupported desktop lifecycle action: ${String(body.action ?? "")}`,
                );
              }
              const status = await detectDesktopApp(manifest);
              return sendJson(response, 200, {
                ...status,
                reason: `${manifest.displayName} ${body.action} is not executed in the browser preview`,
              });
            }
            if (route === `${WEB_ASSISTANT_BASE}/desktop-app-launch`) {
              await launchDesktopApp(desktopManifest(body.app));
              return sendJson(response, 200, { launched: true });
            }
            if (route === `${WEB_ASSISTANT_BASE}/launch-desktop`) {
              await launchDesktopApp(DESKTOP_APPS["codex-desktop"]);
              return sendJson(response, 200, { launched: true });
            }
            sendJson(response, 404, { error: "Unknown bridge endpoint" });
          })
          .catch((error) =>
            sendJson(response, 400, {
              error: error instanceof Error ? error.message : String(error),
            }),
          );
      });
    },
  };
}

export default defineConfig(({ command }) => ({
  root: "src",
  plugins: [
    command === "serve" &&
      codeInspectorPlugin({
        bundler: "vite",
      }),
    command === "serve" && webAssistantBridge(),
    react(),
  ].filter(Boolean),
  base: "./",
  build: {
    outDir: "../dist",
    emptyOutDir: true,
  },
  server: {
    port: 3000,
    strictPort: true,
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_"],
}));
