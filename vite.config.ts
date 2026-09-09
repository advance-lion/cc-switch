import path from "node:path";
import { randomUUID } from "node:crypto";
import { spawn } from "node:child_process";
import { promises as fs } from "node:fs";
import os from "node:os";
import readline from "node:readline";
import type { IncomingMessage, ServerResponse } from "node:http";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { codeInspectorPlugin } from "code-inspector-plugin";

const WEB_ASSISTANT_BASE = "/__cc-switch-dev/codex-assistant";
const MAX_REQUEST_LENGTH = 6_000;

type AssistantPlan = {
  title: string;
  summary: string;
  sources: string[];
  steps: Array<{
    label: string;
    description: string;
    requiresNetwork: boolean;
  }>;
  limitations: string[];
  executable?: boolean;
  install?: {
    tool: string;
    displayName: string;
    version: string;
    installLocation: string;
    usesDefaultLocation: boolean;
    officialSource: string;
  };
};

type RegisteredInstall = {
  kind: "npm" | "desktop";
  tool: string;
  packageName?: string;
  displayName: string;
  version: string;
  targetDir: string;
  usesDefaultLocation: boolean;
  officialSource: string;
};

type StoredPlan = {
  request: string;
  targetDir: string;
  plan: AssistantPlan;
  install?: RegisteredInstall;
};

type DesktopAppId = "codex-desktop" | "claude-desktop";
type DesktopLifecycleAction = "install" | "update" | "uninstall";

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

const planSchema = {
  type: "object",
  additionalProperties: false,
  required: ["title", "summary", "sources", "steps", "limitations"],
  properties: {
    title: { type: "string" },
    summary: { type: "string" },
    sources: { type: "array", items: { type: "string" } },
    limitations: { type: "array", items: { type: "string" } },
    steps: {
      type: "array",
      items: {
        type: "object",
        additionalProperties: false,
        required: ["label", "description", "requiresNetwork"],
        properties: {
          label: { type: "string" },
          description: { type: "string" },
          requiresNetwork: { type: "boolean" },
        },
      },
    },
  },
};

const REGISTERED_NPM_INSTALLS: Record<
  string,
  { packageName: string; displayName: string }
> = {
  claude: {
    packageName: "@anthropic-ai/claude-code",
    displayName: "Claude Code",
  },
  codex: { packageName: "@openai/codex", displayName: "Codex" },
  gemini: { packageName: "@google/gemini-cli", displayName: "Gemini CLI" },
  grok: { packageName: "@xai-official/grok", displayName: "Grok Build" },
  opencode: { packageName: "opencode-ai", displayName: "OpenCode" },
  openclaw: { packageName: "openclaw", displayName: "OpenClaw" },
  pi: { packageName: "@earendil-works/pi-coding-agent", displayName: "Pi" },
};

function normalizeInstallVersion(value: unknown) {
  if (
    typeof value !== "string" ||
    !value.trim() ||
    value === "stable" ||
    value === "latest"
  ) {
    return "latest";
  }
  const normalized = value.trim();
  if (!/^[A-Za-z0-9._-]{1,80}$/.test(normalized)) {
    throw new Error("指定版本只能包含字母、数字、点、连字符或下划线");
  }
  return normalized;
}

function webAssistantBridge() {
  const pendingPlans = new Map<string, StoredPlan>();
  const running = new Map<string, ReturnType<typeof spawn>>();
  const managedInstallDirectories = new Map<string, string>();
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
      throw new Error(`Unsupported desktop application: ${String(value ?? "")}`);
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
    reason: "Desktop application management in the web development bridge is supported on Windows only",
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

  const numericVersionParts = (value: string) =>
    value
      .split(/[^0-9]+/)
      .filter(Boolean)
      .map((part) => Number.parseInt(part, 10));

  const versionIsNewer = (latest: string, current: string) => {
    const latestParts = numericVersionParts(latest);
    const currentParts = numericVersionParts(current);
    const length = Math.max(latestParts.length, currentParts.length);
    for (let index = 0; index < length; index += 1) {
      const difference =
        (latestParts[index] ?? 0) - (currentParts[index] ?? 0);
      if (difference !== 0) return difference > 0;
    }
    return false;
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

  const runWingetDesktopAction = async (
    manifest: DesktopAppManifest,
    action: "install" | "update",
  ) => {
    await commandOutput(
      "winget.exe",
      [
        action === "install" ? "install" : "upgrade",
        "--id",
        manifest.wingetId,
        "--exact",
        "--source",
        manifest.wingetSource,
        "--accept-package-agreements",
        "--accept-source-agreements",
        "--silent",
        "--disable-interactivity",
      ],
      "desktop lifecycle action",
    );
  };

  const uninstallDesktopApp = async (
    manifest: DesktopAppManifest,
    packageIdentity: string,
  ) => {
    if (
      !/^[A-Za-z0-9._-]+$/.test(packageIdentity) ||
      !packageIdentity.startsWith(manifest.windowsPackageName)
    ) {
      throw new Error(
        "Refusing to remove an unverified desktop package identity",
      );
    }
    const script = `$pkg = Get-AppxPackage -Name '${manifest.windowsPackageName}'; if ($null -eq $pkg) { exit 0 }; $pkg | Where-Object { $_.PackageFullName -eq '${packageIdentity}' } | Remove-AppxPackage -ErrorAction Stop`;
    await powershellOutput(script, "desktop application uninstall");
  };

  const runDesktopLifecycleAction = async (
    manifest: DesktopAppManifest,
    action: DesktopLifecycleAction,
  ): Promise<DesktopAppStatus> => {
    if (process.platform !== "win32") {
      throw new Error(
        "Automatic desktop lifecycle actions are supported on Windows only",
      );
    }
    const before = await detectDesktopApp(manifest);
    if (action === "install") {
      if (before.installed) {
        throw new Error(`${manifest.displayName} is already installed`);
      }
      await runWingetDesktopAction(manifest, "install");
    } else if (action === "update") {
      if (!before.installed) {
        throw new Error(`${manifest.displayName} is not installed`);
      }
      const latest = await latestDesktopVersion(manifest);
      if (!versionIsNewer(latest, before.version ?? "")) {
        return { ...before, latest_version: latest };
      }
      await runWingetDesktopAction(manifest, "update");
    } else {
      if (!before.installed) return before;
      if (!before.package_identity) {
        throw new Error("Desktop package identity is unavailable");
      }
      await uninstallDesktopApp(manifest, before.package_identity);
    }

    const after = await detectDesktopApp(manifest);
    if (action === "install" && !after.installed) {
      throw new Error(
        `${manifest.displayName} installer completed but the application was not detected`,
      );
    }
    if (
      action === "update" &&
      (!after.installed ||
        (before.version !== null && after.version === before.version))
    ) {
      throw new Error(
        `${manifest.displayName} update completed but the installed version did not change`,
      );
    }
    if (action === "uninstall" && after.installed) {
      throw new Error(
        `${manifest.displayName} uninstall completed but the package is still installed`,
      );
    }
    return after;
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

  const resolveTargetDirectory = async (raw: unknown) => {
    if (typeof raw !== "string" || !raw.trim())
      throw new Error("请先选择安装目录");
    const targetDir = await fs.realpath(raw.trim());
    const stat = await fs.stat(targetDir);
    if (!stat.isDirectory()) throw new Error("安装位置必须是文件夹");
    if (path.parse(targetDir).root === targetDir)
      throw new Error("不能使用系统磁盘根目录");
    if (path.resolve(targetDir) === path.resolve(os.homedir())) {
      throw new Error("不能使用整个用户主目录");
    }
    return targetDir;
  };

  const defaultPlanWorkspace = async () => {
    const directory = path.join(
      os.tmpdir(),
      "cc-switch-codex-assistant",
      "plan-workspace",
    );
    await fs.mkdir(directory, { recursive: true });
    return await fs.realpath(directory);
  };

  const schemaPath = async () => {
    const directory = path.join(os.tmpdir(), "cc-switch-codex-assistant");
    await fs.mkdir(directory, { recursive: true });
    const filename = path.join(directory, "install-plan.schema.json");
    await fs.writeFile(filename, JSON.stringify(planSchema), "utf8");
    return { directory, filename };
  };

  const createPlanPrompt = (
    request: string,
    targetDir: string,
    install?: RegisteredInstall,
  ) =>
    `You are CC Switch's installation planner. Do not install, download, modify files, or run shell commands.\n` +
    `Create a concise installation explanation for the user's request.\n` +
    (install
      ? `This is a registered CC Switch install for ${install.displayName}. The backend, not you, executes a fixed installer after confirmation. Use only this source in your explanation: ${install.officialSource}. The selected installation location is ${install.usesDefaultLocation ? "CC Switch default installation location" : install.targetDir}. Requested version: ${install.version}. Do not suggest another package, URL, installer, or shell command.\n`
      : "This request is not a registered CC Switch install action. Offer safe manual next steps only and do not present it as executable.\n") +
    `If reliable source or install instructions cannot be established, say so in limitations and propose a manual next step.\n` +
    `Never propose deleting existing files, changing CC Switch configuration, changing Codex configuration, elevation, or disabling safety controls.\n` +
    `Return only the JSON object required by the output schema.\n\nUser request:\n${request}`;

  const isValidPlan = (value: unknown): value is AssistantPlan => {
    if (!value || typeof value !== "object") return false;
    const plan = value as Partial<AssistantPlan>;
    return (
      typeof plan.title === "string" &&
      plan.title.trim().length > 0 &&
      typeof plan.summary === "string" &&
      plan.summary.trim().length > 0 &&
      Array.isArray(plan.sources) &&
      Array.isArray(plan.steps) &&
      Array.isArray(plan.limitations)
    );
  };

  const registeredInstall = (
    tool: unknown,
    targetDir: string,
    requestedVersion: unknown,
    customInstallLocation: boolean,
  ): RegisteredInstall | undefined => {
    if (typeof tool !== "string" || !tool) return undefined;
    if (tool === "codex-desktop" || tool === "claude-desktop") {
      const manifest = DESKTOP_APPS[tool];
      if (customInstallLocation) {
        throw new Error(`${manifest.displayName} 由系统安装器管理，不支持自定义安装位置`);
      }
      const version = normalizeInstallVersion(requestedVersion);
      if (version !== "latest") {
        throw new Error(`${manifest.displayName} 的桌面安装器不支持指定版本`);
      }
      return {
        kind: "desktop",
        tool,
        displayName: manifest.displayName,
        version,
        targetDir,
        usesDefaultLocation: true,
        officialSource:
          tool === "codex-desktop"
            ? "https://apps.microsoft.com/detail/9PLM9XGG6VKS"
            : "https://claude.ai/download",
      };
    }
    const entry = REGISTERED_NPM_INSTALLS[tool];
    if (!entry) throw new Error("当前应用不支持 AI 辅助安装");
    const version = normalizeInstallVersion(requestedVersion);
    return {
      kind: "npm",
      tool,
      packageName: entry.packageName,
      displayName: entry.displayName,
      version,
      targetDir,
      usesDefaultLocation: !customInstallLocation,
      officialSource: `https://www.npmjs.com/package/${entry.packageName}`,
    };
  };

  const enrichPlan = (
    plan: AssistantPlan,
    targetDir: string,
    install?: RegisteredInstall,
  ): AssistantPlan => {
    if (!install) {
      return {
        ...plan,
        executable: false,
        limitations: [
          ...plan.limitations,
          "该应用尚未登记受控安装器；可查看建议，但不能由 CC Switch 自动执行。",
        ],
      };
    }
    return {
      ...plan,
      executable: true,
      install: {
        tool: install.tool,
        displayName: install.displayName,
        version: install.version,
        installLocation: install.usesDefaultLocation
          ? "CC Switch default location"
          : targetDir,
        usesDefaultLocation: install.usesDefaultLocation,
        officialSource: install.officialSource,
      },
    };
  };

  const runCodex = (
    runId: string,
    targetDir: string,
    args: string[],
    onClose: (success: boolean) => Promise<void>,
  ) => {
    const child = spawn("codex", args, {
      cwd: targetDir,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    running.set(runId, child);
    emit({ runId, kind: "started" });
    readline
      .createInterface({ input: child.stdout })
      .on("line", (message) => emit({ runId, kind: "log", message }));
    readline
      .createInterface({ input: child.stderr })
      .on("line", (message) => emit({ runId, kind: "stderr", message }));
    child.on("error", (error) =>
      emit({ runId, kind: "stderr", message: error.message }),
    );
    child.on("close", async (code) => {
      running.delete(runId);
      const success = code === 0;
      await onClose(success);
      emit({
        runId,
        kind: "finished",
        message: code === null ? "cancelled" : `exit code: ${code}`,
        success,
      });
    });
  };

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

  const verifyRegisteredInstall = async (install: RegisteredInstall) => {
    if (install.kind === "desktop") {
      const status = await detectDesktopApp(desktopManifest(install.tool));
      if (!status.installed) {
        throw new Error("安装器已结束，但未检测到桌面应用");
      }
      if (!status.version) throw new Error("桌面应用已安装，但无法读取版本");
      return status.version;
    }
    const resolvedExecutable = install.usesDefaultLocation
      ? install.tool
      : process.platform === "win32"
        ? path.join(install.targetDir, `${install.tool}.cmd`)
        : path.join(install.targetDir, "bin", install.tool);
    const output = await probeToolVersion(
      resolvedExecutable,
      install.targetDir,
      install.usesDefaultLocation
        ? process.env
        : {
            ...process.env,
            PATH: `${
              process.platform === "win32"
                ? install.targetDir
                : path.join(install.targetDir, "bin")
            }${path.delimiter}${process.env.PATH ?? ""}`,
          },
    );
    if (!output.success) {
      throw new Error("安装命令已结束，但未检测到可运行的命令行");
    }
    if (!install.usesDefaultLocation) {
      managedInstallDirectories.set(install.tool, install.targetDir);
    }
    return output.output;
  };

  const runRegisteredInstall = (runId: string, install: RegisteredInstall) => {
    const desktop = install.kind === "desktop"
      ? desktopManifest(install.tool)
      : null;
    const executable = desktop ? "winget.exe" : "npm";
    const args = desktop
      ? [
          "install",
          "--id",
          desktop.wingetId,
          "--exact",
          "--source",
          desktop.wingetSource,
          "--accept-package-agreements",
          "--accept-source-agreements",
          "--silent",
          "--disable-interactivity",
        ]
      : install.usesDefaultLocation
        ? ["install", "--global", `${install.packageName}@${install.version}`]
        : [
            "install",
            "--global",
            "--prefix",
            install.targetDir,
            `${install.packageName}@${install.version}`,
          ];
    const child = spawn(
      executable,
      args,
      {
        cwd: install.targetDir,
        stdio: ["ignore", "pipe", "pipe"],
        windowsHide: true,
      },
    );
    running.set(runId, child);
    emit({
      runId,
      kind: "started",
      message: `install: ${install.displayName}`,
    });
    readline
      .createInterface({ input: child.stdout })
      .on("line", (message) => emit({ runId, kind: "log", message }));
    readline
      .createInterface({ input: child.stderr })
      .on("line", (message) => emit({ runId, kind: "stderr", message }));
    child.on("error", (error) =>
      emit({ runId, kind: "stderr", message: error.message }),
    );
    child.on("close", async (code) => {
      running.delete(runId);
      let success = code === 0;
      let message = code === null ? "cancelled" : `exit code: ${code}`;
      if (success) {
        emit({
          runId,
          kind: "log",
          message: "安装命令已结束，正在重新检测版本…",
        });
        try {
          const version = await verifyRegisteredInstall(install);
          message = `${install.displayName} 已安装并验证：${version}`;
          emit({ runId, kind: "log", message });
        } catch (error) {
          success = false;
          message = error instanceof Error ? error.message : String(error);
          emit({ runId, kind: "stderr", message });
        }
      }
      emit({
        runId,
        kind: "finished",
        message,
        success,
      });
    });
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
            let result = await probeToolVersion("codex");
            if (!result.success) {
              const managedDirectory = managedInstallDirectories.get("codex");
              if (managedDirectory) {
                const executable =
                  process.platform === "win32"
                    ? path.join(managedDirectory, "codex.cmd")
                    : path.join(managedDirectory, "bin", "codex");
                result = await probeToolVersion(executable, managedDirectory, {
                  ...process.env,
                  PATH: `${
                    process.platform === "win32"
                      ? managedDirectory
                      : path.join(managedDirectory, "bin")
                  }${path.delimiter}${process.env.PATH ?? ""}`,
                });
              }
            }
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
            if (route === `${WEB_ASSISTANT_BASE}/plan`) {
              if (typeof body.request !== "string" || !body.request.trim())
                throw new Error("请输入需要安装或配置的 Agent");
              if (body.request.length > MAX_REQUEST_LENGTH)
                throw new Error("请求过长");
              const customInstallLocation =
                body.customInstallLocation !== false;
              const targetDir =
                typeof body.tool === "string" && !customInstallLocation
                  ? await defaultPlanWorkspace()
                  : await resolveTargetDirectory(body.targetDir);
              const install = registeredInstall(
                body.tool,
                targetDir,
                body.requestedVersion,
                customInstallLocation,
              );
              const runId = randomUUID();
              const { directory, filename } = await schemaPath();
              const outputPath = path.join(
                directory,
                `plan-output-${runId}.json`,
              );
              runCodex(
                runId,
                targetDir,
                [
                  "exec",
                  "--json",
                  "--ephemeral",
                  "--skip-git-repo-check",
                  "--ignore-rules",
                  "--cd",
                  targetDir,
                  "--sandbox",
                  "read-only",
                  "--output-schema",
                  filename,
                  "--output-last-message",
                  outputPath,
                  createPlanPrompt(body.request.trim(), targetDir, install),
                ],
                async (success) => {
                  if (!success) return;
                  try {
                    const parsed = JSON.parse(
                      await fs.readFile(outputPath, "utf8"),
                    );
                    if (!isValidPlan(parsed))
                      throw new Error("Codex 未返回可用的结构化安装计划");
                    const planId = randomUUID();
                    const plan = enrichPlan(parsed, targetDir, install);
                    pendingPlans.set(planId, {
                      request: body.request.trim(),
                      targetDir,
                      plan,
                      install,
                    });
                    emit({ runId, kind: "plan", planId, plan });
                  } catch (error) {
                    emit({
                      runId,
                      kind: "stderr",
                      message:
                        error instanceof Error ? error.message : String(error),
                    });
                  } finally {
                    await fs.rm(outputPath, { force: true });
                  }
                },
              );
              return sendJson(response, 202, { runId });
            }
            if (route === `${WEB_ASSISTANT_BASE}/execute`) {
              if (typeof body.planId !== "string")
                throw new Error("安装计划不存在或已失效，请重新生成");
              const stored = pendingPlans.get(body.planId);
              if (!stored)
                throw new Error("安装计划不存在或已失效，请重新生成");
              pendingPlans.delete(body.planId);
              if (!stored.install)
                throw new Error(
                  "该计划没有已登记的受控安装器，只能查看建议和复制手动命令",
                );
              const runId = randomUUID();
              runRegisteredInstall(runId, stored.install);
              return sendJson(response, 202, { runId });
            }
            if (route === `${WEB_ASSISTANT_BASE}/cancel`) {
              if (typeof body.runId !== "string")
                throw new Error("没有正在运行的 Codex 任务");
              const child = running.get(body.runId);
              if (!child) throw new Error("没有正在运行的 Codex 任务");
              child.kill();
              return sendJson(response, 200, { cancelled: true });
            }
            if (
              route === `${WEB_ASSISTANT_BASE}/desktop-app-check-updates`
            ) {
              const status = await checkDesktopAppUpdates(
                desktopManifest(body.app),
              );
              return sendJson(response, 200, status);
            }
            if (route === `${WEB_ASSISTANT_BASE}/desktop-app-action`) {
              if (
                body.action !== "install" &&
                body.action !== "update" &&
                body.action !== "uninstall"
              ) {
                throw new Error(
                  `Unsupported desktop lifecycle action: ${String(body.action ?? "")}`,
                );
              }
              const status = await runDesktopLifecycleAction(
                desktopManifest(body.app),
                body.action,
              );
              return sendJson(response, 200, status);
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
