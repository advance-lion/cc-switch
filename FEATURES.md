# CC Switch Dev — 魔改特性清单

> 基于 CC Switch (v3.20.2) 二次开发，保留原版全部功能，额外增加以下特性。

## 一、内置 Codex 助手

原版 CC Switch 只管理 provider 配置，不直接集成 AI 对话。CC Switch Dev 在侧边栏内建了一个 Codex 助手 Dock，可以直接和 Codex CLI 对话。

### 1.1 浮窗 Dock 面板

- 右侧浮窗按钮，点击展开 390px 宽的对话面板
- 支持拖拽定位，记住位置
- 面板内显示：就绪状态检测（CLI 版本 + Provider 配置）、对话区、运行日志
- 面板可从任意页面打开，不切换当前视图

### 1.2 Codex CLI 后端接入

- 通过 codex exec 命令以 danger-full-access 权限运行
- 后端 Rust 管理进程生命周期：启动、读取 stdout/stderr、取消
- 支持流式输出：实时显示 reasoning 和 message 内容
- 审批弹窗：命令执行前弹出 Approval Card，可单次/本会话/全局放行

### 1.3 会话持久化

- 关闭 Dock 面板（点 x）不会终止后端任务
- 任务在后台继续运行，对话上下文保留
- 重新打开面板即可看到完整对话记录
- 首页按钮触发 openRequestId 自动弹出浮窗

### 1.4 未读/运行中指示器

- 浮窗按钮在有任务运行或有未读消息时显示紫色脉动圆点
- 从任意页面可见，无需打开面板即可知道助手状态
- 打开面板后自动清除未读标记

### 1.5 AGENTS.md 知识注入

- 创建会话时自动在工作目录写入 AGENTS.md
- 描述 CC Switch Dev 的项目架构、Provider 体系、关键模块路径
- Codex CLI 启动时自动读取，获得项目上下文
- 零代码改动，Codex 原生支持

## 二、DeepSeek Harness (DSH) 集成

### 2.1 桌面生命周期管理

- 检测 DSH 桌面应用安装状态（bootstrap / unpacked 两种安装方式）
- 启动/停止/重启 DSH 桌面进程
- 就绪检查：等待 HTTP 端口可用后返回
- 跨平台启动脚本，可靠的重启逻辑

### 2.2 Provider 投影

- DSH provider 配置共享投影到 Provider Center
- 统一管理入口，和其他 provider 并列展示
- Codex routing badge 显示当前路由状态

## 三、Provider Center 增强

### 3.1 统一 Provider 管理

- 所有 AI 编码工具（Claude Code / Codex CLI / Gemini CLI / DSH）的 provider 集中管理
- Provider 编辑修复：通用编辑流程，持久化运行时刷新
- 管理删除：删除 provider 后自动重定向，不留空页

### 3.2 Codex OAuth 路由

- requires_openai_auth 标记：takeover 写入时自动盖戳，匹配实时登录状态
- 代理 image edits 端点修复
- Provider 模型 URL 固定（JieKou / Novita Claude 预设）

## 四、Hermes 桌面集成

- 检测 Hermes 桌面应用安装状态（bootstrap / unpacked）
- 桌面生命周期集成：启动、就绪检查、停止
- 与 Provider Center 联动

## 五、安装与发布

- 安装包名：CC Switch Dev（避免与原版 CC Switch 冲突）
- App identifier：com.ccswitch.desktop.dev
- 独立配置目录，不影响原版 CC Switch 的设置
- NSIS 安装包，支持 Windows x64
- GitHub Release 发布：https://github.com/advance-lion/cc-switch/releases

## 六、与原版 CC Switch 的关系

| 维度 | CC Switch (原版) | CC Switch Dev |
|------|-------------------|---------------|
| 定位 | Provider 配置管理器 | Provider 管理 + 内建 AI 助手 |
| Codex 助手 | 无 | 内建浮窗 Dock，完整对话能力 |
| DSH 集成 | 无 | 桌面生命周期 + Provider 投影 |
| Hermes | 无 | 桌面检测 + 生命周期管理 |
| 会话持久化 | N/A | 关闭面板不杀任务，后台持续运行 |
| AGENTS.md | 无 | 自动注入项目上下文 |
| 安装包 | CC Switch | CC Switch Dev |
| 配置目录 | com.ccswitch.desktop | com.ccswitch.desktop.dev |

## 截图说明

以下截图建议从运行中的应用捕获：

1. 首页 + 浮窗按钮 — 展示浮窗在首页的位置
2. Dock 面板展开 — 展示就绪检测 + 对话区 + 日志
3. 对话中 — 展示流式输出和 Approval Card
4. 关闭面板后 — 展示紫色脉动指示器
5. Provider Center — 展示 DSH + Codex routing badge
6. DSH 桌面生命周期 — 展示启动/停止/就绪状态
