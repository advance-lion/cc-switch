<div align="center">

# CC Switch Dev

### CC Switch 魔改版 — Provider 管理 + 内置 Codex AI 助手

基于 [CC Switch](https://github.com/farion1231/cc-switch) v3.20.2 二次开发，保留原版全部功能，新增内置 Codex 助手、DeepSeek Harness 集成、Hermes 桌面管理等特性。

[![Version](https://img.shields.io/badge/version-3.21.0--dev.2-blue)](https://github.com/advance-lion/cc-switch/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](https://github.com/advance-lion/cc-switch/releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)

[下载最新版本](https://github.com/advance-lion/cc-switch/releases/latest) | [特性清单](FEATURES.md)

</div>

---

## 与原版 CC Switch 的区别

| 维度 | CC Switch (原版) | CC Switch Dev |
|------|-------------------|---------------|
| 定位 | Provider 配置管理器 | Provider 管理 + 内建 AI 助手 |
| Codex 助手 | 无 | 内建浮窗 Dock，完整对话能力 |
| DSH 集成 | 无 | 桌面生命周期 + Provider 投影 |
| Hermes | 无 | 桌面检测 + 生命周期管理 |
| 会话持久化 | N/A | 关闭面板不杀任务，后台持续运行 |
| AGENTS.md | 无 | 自动注入项目上下文 |
| 安装包名 | CC Switch | CC Switch Dev |
| 配置目录 | com.ccswitch.desktop | com.ccswitch.desktop.dev |

> CC Switch Dev 与原版 CC Switch 可共存安装，配置目录独立，互不影响。

---

## 核心新特性

### 1. 内置 Codex 助手

右侧浮窗按钮，点击展开对话面板，直接和 Codex CLI 对话。不需要打开终端，不需要切换页面。

![首页浮窗按钮](screenshots/01_home_floating_button.png)

**面板展开后**：顶部显示就绪状态（CLI 版本 + Provider 配置），中间是对话区，底部是运行日志。

![Dock 面板](screenshots/02_dock_panel.png)

**对话进行中**：流式输出实时显示，命令执行前弹出审批卡片。

![对话中](screenshots/03_conversation.png)

#### 会话持久化

关闭 Dock 面板（点 x）不会终止后端任务。任务在后台继续运行，对话上下文保留。重新打开面板即可看到完整对话记录。

#### 未读 / 运行中指示器

浮窗按钮在有任务运行或有未读消息时显示紫色脉动圆点，从任意页面可见。打开面板后自动清除。

![脉动指示器](screenshots/04_indicator.png)

#### AGENTS.md 知识注入

创建会话时自动在工作目录写入 `AGENTS.md`，描述项目架构和关键模块路径。Codex CLI 启动时自动读取，零代码改动。

#### 后端接入

- 通过 `codex exec` 命令以 `danger-full-access` 权限运行
- Rust 后端管理进程生命周期：启动、stdout/stderr 读取、取消
- 审批策略：命令执行前弹出 Approval Card，可单次 / 本会话 / 全局放行

---

### 2. DeepSeek Harness (DSH) 集成

- 检测 DSH 桌面应用安装状态（bootstrap / unpacked）
- 启动 / 停止 / 重启 DSH 桌面进程
- 就绪检查：等待 HTTP 端口可用
- Provider 配置共享投影到 Provider Center

### 3. Provider Center 增强

![Provider Center](screenshots/05_provider_center.png)

- 所有 AI 编码工具的 provider 集中管理（Claude Code / Codex CLI / Gemini CLI / DSH）
- Codex OAuth 路由：`requires_openai_auth` 标记自动盖戳
- Provider 编辑 / 删除流程修复，持久化运行时刷新

### 4. Hermes 桌面集成

- 检测 Hermes 桌面应用安装状态
- 桌面生命周期集成：启动、就绪检查、停止

---

## 安装

### Windows

从 [GitHub Release](https://github.com/advance-lion/cc-switch/releases/latest) 下载 `CC Switch Dev_x.x.x_x64-setup.exe`，双击安装。

CC Switch Dev 与原版 CC Switch 可共存安装，不会互相覆盖。

### 从源码构建

```bash
git clone https://github.com/advance-lion/cc-switch.git
cd cc-switch
pnpm install
pnpm tauri build
```

安装包输出在 `src-tauri/target/release/bundle/nsis/`。

---

## 技术栈

- **前端**：React + TypeScript + Tailwind CSS + Vite
- **后端**：Rust + Tauri v2
- **AI 助手**：Codex CLI (codex exec) + app-server 协议
- **桌面集成**：DSH / Hermes 桌面生命周期管理

---

## 致谢

基于 [CC Switch](https://github.com/farion1231/cc-switch) by [farion1231](https://github.com/farion1231) 二次开发。感谢原版作者的开源贡献。

## License

MIT
