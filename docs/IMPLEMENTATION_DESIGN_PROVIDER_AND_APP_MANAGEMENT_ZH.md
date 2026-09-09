# Provider 中心与应用管理增强补丁：完整实现设计

> 状态：实施设计稿
> 目标分支：`feat/runtime-lifecycle`
> 基线数据库版本：18
> 对应需求：`PATCH_REQUIREMENTS_PROVIDER_AND_APP_MANAGEMENT_ZH.md`

## 1. 设计目标

本补丁在保留 CC Switch 现有 Provider 保存、切换、代理接管和 live 配置写入能力的前提下，增加两条完整业务链路：

1. 从本机多个已安装应用扫描模型服务，导入为统一 Provider，并显式绑定、应用到一个或多个目标应用；
2. 在当前应用页面中完成安装、启动、更新、卸载，并由 Codex CLI 提供受控的辅助安装。

面向用户的产品词汇统一为“应用”“命令行”“桌面端”“模型服务”“已接入”。`Runtime`、`Binding`、`Adapter` 等词仅存在于内部代码和技术文档。

本设计不把现有 `providers` 表废弃。它继续表示“某个应用实际可用的 Provider 投影”，并继续由成熟的 `ProviderService` 负责保存、切换和写入 live 配置。新增领域层只负责编排统一定义、绑定、导入、事务和应用能力。

## 2. 现状与关键差距

### 2.1 可直接复用的能力

- `ProviderService` 已实现应用 Provider 的保存、切换、配置合并和 live 写入。
- Claude、Codex、Gemini、OpenCode、OpenClaw、Hermes、Pi 已存在各自的配置解析或导入逻辑。
- `read_live_provider_settings`、`import_default_config` 和各应用专用 import command 可作为 Adapter 的第一批实现来源。
- 工具检测已覆盖 PATH、常见目录、包管理器、WSL 和 `<tool> --version` 校验。
- `run_tool_lifecycle_action` 已有受支持工具白名单和登记安装命令，可作为“标准安装”的第一版执行器。
- 当前网页开发桥接与 Tauri 都能调用真实 Codex CLI，并提供事件流、取消和日志。

### 2.2 不能继续沿用的部分

当前 `UniversalProvider` 存在以下结构性限制：

- `UniversalProviderApps` 只有 Claude、Codex、Gemini 三个布尔字段；继续增加字段会让每增加一个应用都修改数据模型、UI 和转换函数。
- `UniversalProviderModels` 按应用固化模型字段，无法表达通用模型能力和应用级覆写。
- API Key 明文保存在 `settings.universal_providers` JSON 中。
- 新建或修改统一 Provider 后会立即同步，不能表达“草稿”“待应用”“已漂移”。
- `to_claude_provider`、`to_codex_provider`、`to_gemini_provider` 把转换逻辑固化在核心实体中。
- 没有导入扫描会话、来源追踪、写入预览、并发检测、跨应用应用记录和回滚记录。
- 关闭某个布尔字段会直接删除生成的子 Provider，删除语义过强。

因此，不应通过扩展 `UniversalProviderApps` 来实现本补丁。应引入 Provider Definition + Binding + Adapter 的新领域模型。

## 3. 目标架构

```text
┌──────────────────────────── 前端 ────────────────────────────┐
│ ProviderCenter  ImportWizard  AppStatusCard  CodexAssistant │
└───────────────┬───────────────────────┬──────────────────────┘
                │ typed IPC             │ event stream
┌───────────────▼───────────────────────▼──────────────────────┐
│                    Application Services                      │
│ ProviderDefinitionService  ImportService  BindingService     │
│ ModelCatalogService        LifecycleService AssistantService │
└───────────────┬───────────────────────┬──────────────────────┘
                │                       │
       ┌────────▼────────┐      ┌──────▼─────────────────┐
       │ AppAdapterRegistry│      │ Capability/ActionRegistry│
       │ scan/render/verify│      │ detect/install/launch   │
       └────────┬────────┘      └──────┬─────────────────┘
                │                       │
┌───────────────▼───────────────────────▼──────────────────────┐
│ Existing ProviderService / config modules / lifecycle probes │
│ save projected Provider → switch/apply → atomic live write    │
└───────────────┬───────────────────────┬──────────────────────┘
                │                       │
       ┌────────▼────────┐      ┌──────▼─────────────┐
       │ SQLite metadata │      │ OS secure storage  │
       │ no plaintext key│      │ API keys/tokens    │
       └─────────────────┘      └────────────────────┘
```

### 3.1 分层责任

- **领域实体**：Provider 定义、模型、绑定、来源、导入会话、应用事务和生命周期任务。
- **Adapter**：知道每个应用如何探测、读取、标准化、渲染和验证配置；不决定 UI，不直接管理跨应用事务。
- **Application Service**：执行业务流程、权限检查、状态转换、幂等和回滚。
- **现有 ProviderService**：保存应用级投影，并使用现有安全规则写入 live 配置。
- **SecretStore**：唯一允许持久化和读取 API Key 的组件。
- **前端**：只接收 `credentialConfigured: boolean`、尾部掩码等非敏感状态，永不接收完整密钥。

## 4. 领域模型

### 4.1 ProviderDefinition

代表一份与具体应用无关的模型服务定义。

```rust
pub struct ProviderDefinition {
    pub id: String,
    pub name: String,
    pub provider_kind: String,
    pub protocol: ProviderProtocol,
    pub base_url: String,
    pub secret_ref: Option<String>,
    pub website_url: Option<String>,
    pub notes: Option<String>,
    pub icon: Option<String>,
    pub icon_color: Option<String>,
    pub metadata: serde_json::Value,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}
```

`ProviderProtocol` 第一版建议支持：

- `openai_responses`
- `openai_chat_completions`
- `anthropic_messages`
- `gemini_generate_content`
- `ollama`
- `custom`

`metadata` 只能保存非敏感扩展。数据库和前端 DTO 中不得出现 `api_key`。

### 4.2 ProviderSecret

API Key 不建 SQLite 明文表。数据库只保存不可逆推导密钥的 `secret_ref`，例如：

```text
cc-switch/provider/<provider-id>/api-key
```

后端定义统一接口：

```rust
pub trait SecretStore: Send + Sync {
    fn put(&self, reference: &str, value: SecretString) -> Result<(), AppError>;
    fn get(&self, reference: &str) -> Result<Option<SecretString>, AppError>;
    fn delete(&self, reference: &str) -> Result<(), AppError>;
    fn move_secret(&self, from: &str, to: &str) -> Result<(), AppError>;
}
```

建议使用 Rust `keyring` crate 接入 Windows Credential Manager、macOS Keychain 和 Linux Secret Service。若系统安全存储不可用，不得降级为 SQLite 或日志中的明文；允许“本次使用但不保存”，并向用户明确提示。

### 4.3 ProviderModel

统一模型目录按 Provider 保存，不再按目标应用保存三套固定字段。

```rust
pub struct ProviderModel {
    pub provider_id: String,
    pub model_id: String,
    pub display_name: Option<String>,
    pub enabled: bool,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    pub reasoning_levels: Vec<String>,
    pub context_window: Option<i64>,
    pub metadata: serde_json::Value,
    pub sort_order: i64,
}
```

模型接口拉取结果先进入预览，用户勾选后才写入 `provider_models`。目标应用不支持的模型能力由 Adapter 在预览中标记，而不是静默丢弃。

### 4.4 ProviderBinding

Binding 表示“把某个统一 Provider 投射给某个目标应用”。

```rust
pub enum BindingState {
    Draft,
    PendingApply,
    Applied,
    Drifted,
    Failed,
    Disabled,
}

pub struct ProviderBinding {
    pub id: String,
    pub provider_id: String,
    pub app_type: AppType,
    pub state: BindingState,
    pub adapter_id: String,
    pub adapter_version: i64,
    pub projected_provider_id: Option<String>,
    pub overrides: serde_json::Value,
    pub desired_revision: i64,
    pub applied_revision: Option<i64>,
    pub projection_digest: Option<String>,
    pub last_apply_transaction_id: Option<String>,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
```

状态转换：

```text
draft ──用户勾选启用──> pending_apply ──应用成功──> applied
  │                           │                         │
  └──删除草稿                 └──失败──> failed         └──定义变化/外部修改──> drifted
                                                            │
                              disabled <──用户停用───────────┘
```

规则：

- 修改 ProviderDefinition 时，只把相关 Binding 标成 `pending_apply`，不自动写目标配置。
- `applied_revision != desired_revision` 必须显示“待同步”。
- 禁用 Binding 默认只停止后续同步；是否移除已生成的应用 Provider 必须另行确认。
- 删除 ProviderDefinition 前必须处理现有 Binding，不能隐式级联删除目标应用中的 Provider。

### 4.5 ProviderSource

记录导入来源，用于展示和漂移检测，但来源始终只读。

字段包括：`provider_id`、`source_app_type`、`source_provider_id`、`source_locator`、`source_fingerprint`、`imported_at`、`last_observed_at`。

`source_fingerprint` 对去除密钥后的规范化配置计算 SHA-256。`source_locator` 只保存必要路径信息；前端默认显示应用名称而非完整用户目录。

### 4.6 ImportSession 与 ImportCandidate

扫描不是直接导入。一次扫描创建有过期时间的会话：

- `ImportSession`：状态、创建时间、过期时间、扫描的应用集合、错误摘要。
- `ImportCandidate`：来源、标准化非敏感配置、模型、是否有密钥、临时 `secret_ref`、冲突信息。

扫描出的密钥立即放入安全存储的临时命名空间，例如 `cc-switch/import/<session>/<candidate>`；SQLite 只记录引用。会话取消或过期后删除临时密钥。

### 4.7 ApplyTransaction

每次“启用并应用”都创建事务记录：

- 主记录：事务 ID、发起原因、状态、开始/结束时间、幂等键。
- 目标记录：Binding ID、应用、应用前文件指纹、应用后文件指纹、快照引用、结果和错误。

快照内容沿用或扩展现有配置备份设施。快照可能包含目标应用自身的密钥，因此必须按敏感数据处理：优先加密保存，日志和前端只返回摘要。

### 4.8 AppCapabilityManifest 与 LifecycleJob

每个支持的应用声明可验证的能力，而不是让 UI 或 AI 猜测：

```rust
pub struct AppCapabilityManifest {
    pub app_id: String,
    pub components: Vec<AppComponentCapability>, // cli / desktop
    pub install_methods: Vec<InstallMethod>,
    pub supports_custom_directory: bool,
    pub supports_version_pin: bool,
    pub supports_update: bool,
    pub supports_uninstall: bool,
    pub supports_launch: bool,
    pub adapter_id: String,
}
```

`LifecycleJob` 保存任务状态、当前步骤、耗时、取消状态、安装前后探测结果和脱敏日志。CLI 与 Desktop 是两个 component，尤其 Codex 不得合并成一个状态。

## 5. SQLite v19 迁移建议

将 `SCHEMA_VERSION` 从 18 升到 19，并在 `schema.rs` 添加 `migrate_v18_to_v19`。建议表结构如下；字段名可以按现有 Rust DAO 风格微调。

```sql
CREATE TABLE provider_definitions (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  provider_kind TEXT NOT NULL,
  protocol TEXT NOT NULL,
  base_url TEXT NOT NULL,
  secret_ref TEXT,
  website_url TEXT,
  notes TEXT,
  icon TEXT,
  icon_color TEXT,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  revision INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE provider_models (
  provider_id TEXT NOT NULL,
  model_id TEXT NOT NULL,
  display_name TEXT,
  enabled INTEGER NOT NULL DEFAULT 1,
  input_modalities_json TEXT NOT NULL DEFAULT '[]',
  output_modalities_json TEXT NOT NULL DEFAULT '[]',
  reasoning_levels_json TEXT NOT NULL DEFAULT '[]',
  context_window INTEGER,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  sort_order INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (provider_id, model_id),
  FOREIGN KEY (provider_id) REFERENCES provider_definitions(id) ON DELETE CASCADE
);

CREATE TABLE provider_bindings (
  id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL,
  app_type TEXT NOT NULL,
  state TEXT NOT NULL,
  adapter_id TEXT NOT NULL,
  adapter_version INTEGER NOT NULL,
  projected_provider_id TEXT,
  overrides_json TEXT NOT NULL DEFAULT '{}',
  desired_revision INTEGER NOT NULL,
  applied_revision INTEGER,
  projection_digest TEXT,
  last_apply_transaction_id TEXT,
  last_error_code TEXT,
  last_error_message TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (provider_id, app_type),
  FOREIGN KEY (provider_id) REFERENCES provider_definitions(id) ON DELETE RESTRICT
);

CREATE TABLE provider_sources (
  id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL,
  source_app_type TEXT NOT NULL,
  source_provider_id TEXT,
  source_locator TEXT,
  source_fingerprint TEXT NOT NULL,
  imported_at INTEGER NOT NULL,
  last_observed_at INTEGER,
  FOREIGN KEY (provider_id) REFERENCES provider_definitions(id) ON DELETE CASCADE
);

CREATE TABLE import_sessions (
  id TEXT PRIMARY KEY,
  state TEXT NOT NULL,
  requested_apps_json TEXT NOT NULL,
  error_summary_json TEXT NOT NULL DEFAULT '[]',
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  completed_at INTEGER
);

CREATE TABLE import_candidates (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  source_app_type TEXT NOT NULL,
  source_provider_id TEXT,
  source_locator TEXT,
  normalized_json TEXT NOT NULL,
  models_json TEXT NOT NULL DEFAULT '[]',
  temporary_secret_ref TEXT,
  credential_configured INTEGER NOT NULL DEFAULT 0,
  fingerprint TEXT NOT NULL,
  conflict_json TEXT,
  FOREIGN KEY (session_id) REFERENCES import_sessions(id) ON DELETE CASCADE
);

CREATE TABLE apply_transactions (
  id TEXT PRIMARY KEY,
  reason TEXT NOT NULL,
  state TEXT NOT NULL,
  idempotency_key TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  completed_at INTEGER
);

CREATE TABLE apply_transaction_targets (
  transaction_id TEXT NOT NULL,
  binding_id TEXT NOT NULL,
  state TEXT NOT NULL,
  before_fingerprint TEXT,
  after_fingerprint TEXT,
  snapshot_ref TEXT,
  error_code TEXT,
  error_message TEXT,
  PRIMARY KEY (transaction_id, binding_id),
  FOREIGN KEY (transaction_id) REFERENCES apply_transactions(id) ON DELETE CASCADE,
  FOREIGN KEY (binding_id) REFERENCES provider_bindings(id) ON DELETE RESTRICT
);

CREATE TABLE lifecycle_jobs (
  id TEXT PRIMARY KEY,
  app_id TEXT NOT NULL,
  component TEXT NOT NULL,
  action TEXT NOT NULL,
  state TEXT NOT NULL,
  plan_json TEXT NOT NULL,
  pre_probe_json TEXT,
  post_probe_json TEXT,
  error_code TEXT,
  error_message TEXT,
  created_at INTEGER NOT NULL,
  started_at INTEGER,
  completed_at INTEGER
);
```

必要索引：Binding 的 `app_type/state`、source 的 `source_app_type`、session 的 `expires_at`、job 的 `state/created_at`。

### 5.1 旧 Universal Provider 迁移

SQLite schema migration 只建表，不能在事务中依赖 OS 密钥环。应用启动后由可重试的 `LegacyUniversalProviderMigrator` 完成数据迁移：

1. 读取 `settings.universal_providers`。
2. 为每项创建 ProviderDefinition。
3. 把 API Key 写入 OS 安全存储，成功后只保存 `secret_ref`。
4. 将 Claude/Codex/Gemini 的 true 字段转换为 Binding；已有 `universal-<app>-<id>` 子 Provider 记录到 `projected_provider_id`。
5. 模型字段转换为 `provider_models` 和 Binding overrides。
6. 所有条目成功后删除旧 JSON，并写入迁移完成标记。

迁移必须幂等。任何一项密钥写入失败时保留旧 JSON、回滚本轮新增记录并提示用户，不能先删明文再发现安全存储失败。为兼容降级版本，建议仅在新版本稳定后删除旧键；开发阶段可以先备份数据库。

## 6. AppAdapter 设计

### 6.1 统一接口

```rust
#[async_trait]
pub trait AppAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn version(&self) -> i64;
    fn app_type(&self) -> AppType;
    fn capabilities(&self) -> AdapterCapabilities;

    async fn detect(&self, ctx: &AdapterContext) -> Result<AppDetection, AppError>;
    async fn scan_providers(&self, ctx: &AdapterContext) -> Result<Vec<ScannedProvider>, AppError>;
    fn normalize(&self, source: RawProvider) -> Result<NormalizedProvider, AppError>;
    fn validate_binding(&self, input: &BindingInput) -> Vec<ValidationIssue>;
    fn render_projection(&self, input: &BindingInput, secret: Option<&SecretString>)
        -> Result<ProviderProjection, AppError>;
    fn read_live_fingerprint(&self, ctx: &AdapterContext) -> Result<String, AppError>;
    fn verify_projection(&self, ctx: &AdapterContext, expected: &ProviderProjection)
        -> Result<VerificationResult, AppError>;
}
```

Adapter 只生成标准 `ProviderProjection`，最终保存和 live 写入仍交给 `ProviderService`。这样保留现有代理接管、当前 Provider、additive/switch 模式和原子文件写入规则。

### 6.2 第一批 Adapter

| Adapter | 扫描来源 | 投影复用点 |
| --- | --- | --- |
| Claude Code | settings/env | Claude Provider 构造与 `ProviderService` |
| Codex CLI | config.toml + auth.json | `codex_config` |
| Gemini CLI | settings/env | Gemini 配置转换 |
| OpenCode | opencode.json/jsonc | `import_opencode_providers_from_live`、`opencode_config` |
| OpenClaw | openclaw.json | `import_openclaw_providers_from_live`、`openclaw_config` |
| Hermes | config.yaml | `import_hermes_providers_from_live`、`hermes_config` |
| Pi | models.json | Pi provider service、`pi_config` |
| Claude Desktop | Desktop profile | `claude_desktop_config` |

ZCode、DSH 等新增应用通过新的 Adapter 和能力清单加入，不修改 ProviderDefinition 表。若它们尚未进入 `AppType`，应先扩展 `app_config.rs`、路径配置、导航和 ProviderService 支持，再注册 Adapter。

### 6.3 Adapter 注册表

```rust
pub struct AppAdapterRegistry {
    adapters: HashMap<AppType, Arc<dyn AppAdapter>>,
}
```

应用启动时完成静态注册。IPC 只接收 `app_type`，不接收类名、脚本或任意可执行路径。未注册的应用返回 `APP_NOT_SUPPORTED`。

## 7. 导入、绑定与应用流程

### 7.1 扫描与预览

```text
用户点击“导入电脑已有配置”
  → ImportService 创建 session
  → 并发调用已登记且已安装的 Adapter.scan_providers
  → 密钥写入临时 SecretStore，标准化结果写入 candidates
  → 前端收到脱敏候选：来源、名称、URL、协议、模型、密钥是否存在
  → 用户选择候选和目标应用
  → Adapter.validate_binding + render_projection
  → 前端展示逐应用预览、冲突、会覆盖/保留的字段
```

扫描失败必须按应用隔离。一个应用配置损坏不能让其他应用结果消失。

### 7.2 提交导入

`commit_import_session` 在一个数据库事务中：

1. 校验 session 未过期且候选未变化；
2. 创建或合并 ProviderDefinition；
3. 将临时密钥移动到正式 secret ref；
4. 保存 ProviderModel 和 ProviderSource；
5. 为目标应用创建 `pending_apply` Binding；
6. 返回应用预览，不写任何目标 live 配置。

冲突策略必须由用户选择：

- 新建副本；
- 合并到已有统一 Provider；
- 跳过。

不得仅按 Provider 名称自动覆盖。可用规范化 Base URL + protocol + source fingerprint 提供“可能相同”的建议。

### 7.3 启用并应用

```text
用户确认
  → 创建 ApplyTransaction（idempotency key）
  → 锁定涉及的 app_type
  → 重新读取 live fingerprint，发现变化则停止并重新预览
  → 为全部目标创建快照
  → 逐 Binding 解析密钥、render projection
  → 保存/更新现有 providers 表中的应用级 Provider
  → 调用 ProviderService 写入目标 live 配置
  → Adapter.verify_projection
  → 全部成功：Binding=applied，记录 revision/digest
  → 任一失败：回滚已写目标；失败或无法完整回滚者标记 failed/drifted
```

默认采用“尽力原子”的批量语义：写入前全部完成预检和快照；任一目标失败时回滚本事务已修改的目标。跨多个独立配置文件无法做到数据库级真正原子，因此 UI 必须展示逐目标结果和可能需要人工处理的项。

### 7.4 漂移检测

以下情况将 Binding 标为 `drifted`：

- 目标应用的受管投影被外部修改；
- Adapter 版本升级后重新渲染的 digest 不同；
- 应用 live 配置不再包含预期 Provider；
- 回滚或验证失败。

“重新应用”之前必须重新生成预览。不得用旧快照盲目覆盖用户在其他工具中的新修改。

## 8. 模型目录与账号分离

`UnifiedModelCatalogService` 的输入包括：

- 当前应用的 `applied` Binding；
- 该 Binding 对应 Provider 中 `enabled=true` 的模型；
- Adapter 对协议和模型能力的过滤结果；
- 应用原生账号登录提供的模型目录。

输出只包含当前应用实际可用的模型，并标记来源：`api_key_provider`、`native_account` 或 `coding_plan`。

账号登录和 Coding Plan 不转换为 ProviderDefinition，不共享 API Key，也不进入 Provider 导入流程。UI 可以在同一页面分区展示，但后端实体和 API 必须分开。

## 9. 应用安装与管理架构

### 9.1 组件化状态

应用状态按 component 探测：

- `cli`
- `desktop`
- 必要时 `background_service`

Codex 页面同时请求 Codex CLI 与 Codex Desktop，分别显示路径、版本、来源和操作。CLI 的“启动”是打开终端；Desktop 的“启动”才是启动桌面程序。

### 9.2 标准安装

标准安装继续使用登记映射，但从 `misc.rs` 中逐步抽离为 `LifecycleRegistry`：

```rust
pub trait LifecycleAdapter {
    fn detect(&self, component: AppComponent) -> Result<InstallationProbe, AppError>;
    fn plan(&self, request: LifecycleRequest) -> Result<ValidatedActionPlan, AppError>;
    async fn execute(&self, plan: ValidatedActionPlan, sink: JobEventSink)
        -> Result<(), AppError>;
    fn verify(&self, before: &InstallationProbe) -> Result<InstallationProbe, AppError>;
}
```

安装、更新、卸载都必须做后置探测：

- 安装：必须出现可运行路径和版本；
- 更新：版本应达到计划目标，不能只依据退出码；
- 卸载：受管安装实例必须消失；若仍检测到其他来源，显示“仍发现另一份安装”而非失败或误报成功。

安装来源未知时，启动和接入仍可用，更新与卸载只给人工指引。

### 9.3 任务状态机

```text
draft → awaiting_confirmation → queued → running → verifying → succeeded
                              └──────────────→ cancelling → cancelled
                              └──────────────→ failed
                                          verifying 失败 → failed_verification
```

每一步产生结构化事件：`job_started`、`step_started`、`log`、`step_finished`、`verification`、`job_finished`。页面刷新后可用 job ID 恢复状态，不能仅依赖组件内存。

卸载只删除所选 component，不默认删除 Provider、Binding、账号、配置快照或其他来源安装。

## 10. Codex CLI 助手的安全执行边界

当前 `codex_assistant.rs` 已具备计划、确认、事件流、取消和安装目录限制，可作为交互与任务框架。但最终执行层必须从“让 Codex 根据自然语言执行”收紧为“Codex 只选择后端允许的结构化动作”。

### 10.1 允许的职责

Codex CLI 可以：

- 理解用户意图并选择已登记的应用、component、版本和安装方式；
- 解释为什么选择该方案；
- 生成符合 JSON Schema 的 `ProposedActionPlan`；
- 在失败后根据结构化错误建议下一步；
- 对未支持应用生成说明和可复制命令，但不能执行。

Codex CLI 不可以：

- 返回任意 Shell 字符串供后端直接执行；
- 自由选择下载域名、可执行文件或脚本 URL；
- 越过用户选择的安装目录；
- 修改 Provider、账号或非目标应用配置；
- 将 API Key 写入 prompt、stdout、日志或临时明文文件。

### 10.2 结构化动作

建议的可执行动作枚举：

```rust
pub enum InstallerAction {
    EnsureDirectory { directory_id: String },
    DownloadRegisteredArtifact { artifact_id: String, version: String },
    VerifyChecksum { artifact_id: String },
    RunRegisteredInstaller { installer_id: String, args: BTreeMap<String, String> },
    InstallRegisteredPackage { package_id: String, version: String, prefix_id: Option<String> },
    VerifyRegisteredProbe { probe_id: String },
    LaunchRegisteredTarget { launch_id: String },
}
```

后端使用 `CapabilityRegistry` 做二次验证：动作 ID 必须存在、来源必须匹配 manifest、参数值必须满足枚举或路径约束、版本必须符合策略。计划包含未知动作时整份计划拒绝执行。

### 10.3 自定义 Agent（例如 deepseek-harness）

“+ 添加新应用”可让 Codex 辅助生成 `ProposedAppManifest`，但生成结果只进入预览：

1. Codex 查明官方仓库、包名、可执行名和版本命令；
2. 用户查看来源与声明能力；
3. 后端验证 URL、命令模板和参数 schema；
4. 用户明确保存为本地应用清单；
5. 安装仍只通过清单注册后的动作执行。

未经审核的清单只能显示说明和复制命令。后续若要共享自定义清单，应增加签名、版本和来源信任机制，不能把任意 JSON 当成可信安装器。

## 11. IPC/API 契约

IPC 按业务动作设计，禁止前端提交文件路径或 Shell 命令来绕过 Adapter。

### 11.1 Provider 与模型

```text
list_provider_definitions()
get_provider_definition(id)
create_provider_definition(input_with_optional_secret)
update_provider_definition(id, expected_revision, patch_with_optional_secret)
delete_provider_definition(id, mode)
fetch_provider_models(id)
replace_provider_models(id, expected_revision, models)
```

响应 DTO 只包含：`credentialConfigured`、可选 `credentialHint`，不包含 secret ref 和密钥明文。

### 11.2 导入与绑定

```text
start_provider_scan(apps) -> session_id
list_import_candidates(session_id)
preview_import(session_id, selections, target_apps)
commit_import_session(session_id, selections, conflict_decisions)
list_provider_bindings(provider_id?)
preview_binding_apply(binding_ids)
apply_provider_bindings(binding_ids, preview_token, idempotency_key)
disable_provider_binding(binding_id, remove_projection: bool)
restore_apply_transaction(transaction_id)
```

`preview_token` 绑定 definition revision、Adapter version 和 live fingerprint。任一输入变化后 token 失效，防止“看见 A、实际写入 B”。

### 11.3 生命周期与助手

```text
get_app_capabilities(app_id)
probe_app_components(app_id)
plan_lifecycle_action(request)
start_lifecycle_job(validated_plan_id)
cancel_lifecycle_job(job_id)
get_lifecycle_job(job_id)
list_recent_lifecycle_jobs(app_id)
propose_custom_app_manifest(request)
validate_custom_app_manifest(manifest)
```

事件统一携带 `jobId`、`sequence` 和时间戳。前端按 sequence 去重并支持断线后从最后序号恢复。

## 12. 前端信息架构

### 12.1 Provider 中心

建议把现有 `UniversalProviderPanel` 逐步替换为：

- `ProviderCenterPage`
- `ProviderDefinitionList`
- `ProviderDefinitionEditor`
- `ProviderModelPicker`
- `ProviderBindingMatrix`
- `ImportProviderWizard`
- `ApplyPreviewDialog`

Provider 卡片显示名称、URL、协议、密钥是否配置、模型数、来源和绑定摘要。修改后显示“有更改待应用”，不自动同步。

### 12.2 应用页面

`RuntimeLifecycleCard` 可保留内部文件名一段时间，但用户文案全部改为“应用状态”。后续建议重命名为 `AppLifecycleCard`。

状态卡保持在 Provider 页面顶部 `sticky`，包括：

- 应用图标和安装状态；
- CLI/Desktop 独立版本；
- 接入状态；
- 标准安装、AI 辅助安装、启动、更新、卸载、重新检测；
- 任务进行中的步骤、耗时、停止和日志入口。

“AI 辅助安装”只在 Codex CLI 可用且模型配置可用时启用；不可用时仍显示按钮和明确的配置引导，不能直接消失。

### 12.3 Codex 助手

- 默认吸附右侧，可拖动，使用 `requestAnimationFrame + translate3d`。
- 每次应用启动/组件首次挂载显示问候气泡。
- 气泡位于图标左侧、文字左对齐、半透明但保持可读。
- 右上角提供叉号；关闭只影响本次启动，不写长期“永不提示”。
- 点击助手图标同时关闭气泡并打开右侧面板。
- 圆形按钮只比 Codex 图标大一圈，保留至少 34px 的可点击区域和键盘焦点样式。

### 12.4 前端状态管理

使用现有 TanStack Query：

- Query：definitions、bindings、models、scan session、component probes、jobs。
- Mutation 成功后按领域事件精确 invalidate。
- 任务日志用事件流增量更新；最终事件触发 probe 和 query refresh。
- 不把密钥、完整配置快照或可执行命令放入 Query cache、localStorage 或错误 toast。

## 13. 并发、幂等与恢复

- ProviderDefinition 使用 `revision` 做乐观并发；更新 API 必须带 `expected_revision`。
- 应用写入使用按 `app_type` 的异步互斥锁，Provider 切换、Binding apply 和恢复操作共用同一把锁。
- 写 live 文件前读取 fingerprint；与预览不一致则返回 `LIVE_CONFIG_CHANGED`。
- 文件写入使用同目录临时文件、flush、原子 rename，并保留权限位。
- apply 和 lifecycle start 接收客户端生成的幂等键；重复请求返回原 transaction/job。
- 应用启动时把遗留 `running/verifying` 任务标记为 `interrupted`，重新探测后给出恢复或重试选项。
- 临时 import secret、过期 session 和旧日志由后台清理任务回收。

## 14. 安全要求

- API Key 只在 Rust 后端和 SecretStore 之间流转；前端永不读取明文。
- 所有日志进入统一脱敏器；URL 的 userinfo、query token、Authorization、已知 secret 都要替换。
- 导入候选和预览禁止包含密钥，配置 diff 中密钥字段固定显示 `••••••••`。
- Assistant prompt 不包含密钥；执行器按 provider ID 在最后一步取密钥。
- 下载仅允许 HTTPS 官方来源；登记 artifact 必须校验 checksum 或可信签名。
- 防止路径逃逸：自定义安装目录 canonicalize 后必须位于用户选择目录内；拒绝设备路径、UNC（除已支持 WSL 流程）、符号链接逃逸和系统关键目录。
- Adapter 解析不可信 JSON/TOML/YAML 时限制文件大小和递归深度。
- 所有 destructive 操作列出确切 component、来源和路径并二次确认。
- 默认导出不含密钥；含密钥导出需单独授权和加密容器。

## 15. 错误码与可观测性

不要只向前端返回拼接字符串。核心错误码至少包括：

- `APP_NOT_SUPPORTED`
- `APP_NOT_INSTALLED`
- `SOURCE_CONFIG_INVALID`
- `SECRET_STORE_UNAVAILABLE`
- `SECRET_NOT_FOUND`
- `IMPORT_SESSION_EXPIRED`
- `IMPORT_SOURCE_CHANGED`
- `PROVIDER_REVISION_CONFLICT`
- `BINDING_VALIDATION_FAILED`
- `LIVE_CONFIG_CHANGED`
- `APPLY_FAILED`
- `ROLLBACK_INCOMPLETE`
- `INSTALL_SOURCE_UNKNOWN`
- `PLAN_NOT_ALLOWED`
- `POST_INSTALL_VERIFICATION_FAILED`
- `JOB_CANCELLED`

Rust 日志记录 transaction/job ID、app、Adapter、阶段、耗时和错误码；不记录密钥、完整 URL 查询串、完整配置或用户 prompt 中可能的敏感内容。

## 16. 文件级改造清单

### 16.1 后端新增

```text
src-tauri/src/provider_center/
  mod.rs
  models.rs
  service.rs
  import_service.rs
  binding_service.rs
  model_catalog.rs
  secret_store.rs
  transaction.rs
  adapters/
    mod.rs
    claude.rs
    codex.rs
    gemini.rs
    opencode.rs
    openclaw.rs
    hermes.rs
    pi.rs
    claude_desktop.rs

src-tauri/src/lifecycle/
  mod.rs
  manifest.rs
  registry.rs
  service.rs
  job.rs
  executor.rs
  probes.rs

src-tauri/src/database/dao/
  provider_definitions.rs
  provider_bindings.rs
  provider_imports.rs
  apply_transactions.rs
  lifecycle_jobs.rs

src-tauri/src/commands/
  provider_center.rs
  lifecycle.rs
```

### 16.2 后端修改

- `database/mod.rs`：版本升到 19，挂载 DAO。
- `database/schema.rs`：新建表、索引与迁移测试。
- `provider.rs`：旧 Universal 类型标记 deprecated；不要继续扩字段。
- `services/provider/mod.rs`：抽出可被 BindingService 调用的投影保存/应用入口；保留现有 live 规则。
- `commands/provider.rs`：旧 Universal IPC 保留兼容层，内部转调新服务。
- `commands/misc.rs`：生命周期映射逐步移入 registry，旧 command 暂作兼容 facade。
- `commands/codex_assistant.rs`：执行阶段改为只接受 validated action plan。
- `lib.rs`：注册新 command、服务状态和事件。
- `Cargo.toml`：增加安全存储依赖及必要的平台 feature。

### 16.3 前端新增/修改

```text
src/components/provider-center/
  ProviderCenterPage.tsx
  ProviderDefinitionCard.tsx
  ProviderDefinitionEditor.tsx
  ProviderBindingMatrix.tsx
  ProviderModelPicker.tsx
  ImportProviderWizard.tsx
  ApplyPreviewDialog.tsx

src/components/app-management/
  AppLifecycleCard.tsx
  ComponentStatusRow.tsx
  LifecycleJobPanel.tsx
  UninstallDialog.tsx

src/lib/api/providerCenter.ts
src/lib/api/lifecycle.ts
src/lib/query/providerCenter.ts
src/lib/query/lifecycle.ts
src/types/providerCenter.ts
src/types/lifecycle.ts
```

- `App.tsx`：挂载 Provider Center、应用状态卡和助手。
- `UniversalProviderPanel.tsx`：先做兼容入口，最终替换。
- `RuntimeLifecycleCard.tsx`：重命名并拆分 component 行。
- `CodexAssistantDock.tsx`：对接结构化 lifecycle plan/job。
- 四种 locale：同步新增文案，新增用户界面不得出现 Runtime。
- `vite.config.ts`：开发桥接复用正式 DTO 和校验器，不能形成安全语义不同的第二套实现。

## 17. 分阶段实施顺序

### 阶段 A：稳定当前 UI 与生命周期探测

- 完成气泡、应用状态卡、Codex CLI/Desktop 分栏。
- 标准安装后置探测、启动、更新、卸载错误语义统一。
- 为现有 lifecycle command 增加任务 ID、日志和取消能力。

### 阶段 B：新数据层与 SecretStore

- SQLite v19 表和 DAO。
- SecretStore 平台实现、脱敏 DTO、密钥迁移测试。
- 旧 Universal Provider 启动后迁移器与兼容读取。

### 阶段 C：Adapter 与只读扫描

- 先实现 Claude/Codex/Gemini，再实现 OpenCode/OpenClaw/Hermes/Pi。
- 导入会话、候选预览、冲突检测。
- 此阶段禁止写目标 live 配置。

### 阶段 D：Binding 应用与回滚

- Binding 状态机、应用预览 token、ApplyTransaction。
- 接入现有 ProviderService。
- 漂移检测、失败回滚和逐目标结果 UI。

### 阶段 E：统一模型目录

- Provider 模型拉取与选择。
- 目标应用能力过滤。
- 账号/Coding Plan 与 API Key Provider 分区。

### 阶段 F：受控 AI 安装

- Capability manifest 和结构化 action schema。
- Codex 只生成提案，后端注册表验证和执行。
- 自定义应用清单预览、校验和受限保存。

每个阶段均应能独立发布；不能等所有 Adapter 完成后才验证数据安全和回滚。

## 18. 测试矩阵

### 18.1 单元测试

- 每个 Adapter 的 normalize/render/verify golden tests。
- Provider revision 和 Binding 状态转换。
- secret DTO 序列化确保没有明文与 secret ref。
- 路径 canonicalize、来源白名单、动作参数 schema。
- 配置 fingerprint 对字段顺序稳定、对密钥变化不泄漏。
- 生命周期版本比较、安装多实例和未知来源。

### 18.2 数据库与迁移测试

- 全新数据库直接创建 v19。
- v18 含/不含 Universal Provider 的升级。
- 密钥环成功、失败、重试、部分成功后崩溃。
- 旧子 Provider 映射后不重复创建。
- 降级保护、外键、删除限制和幂等键。

### 18.3 集成测试

- 扫描多个应用，其中一个配置损坏。
- 导入只复制，来源文件 hash 不变。
- Provider 修改只产生 pending，不写 live。
- 多目标 apply 成功、第二目标失败并回滚第一目标。
- 预览后外部修改 live 文件，apply 被拒绝。
- 密钥不存在时不生成残缺 live 配置。
- Codex CLI 与 Desktop 分别安装、探测、启动和卸载。
- 安装命令退出 0 但版本未变化时返回验证失败。
- 未登记 AI 动作、非官方 URL、越界路径全部拒绝。

### 18.4 前端测试

- Import Wizard 的选择、冲突、预览、失败恢复。
- 待应用、已应用、漂移、失败状态。
- AI 按钮在未配置时可见但禁用并提供引导。
- 气泡每次挂载显示、左对齐、可关闭、点击助手后隐藏。
- 拖动流畅、窗口缩放后仍在视口内、键盘可访问、减少动画模式。
- 所有新增界面文案扫描不含 `Runtime`。

### 18.5 平台矩阵

- Windows：npm/nvm/fnm/volta/mise、独立安装器、Windows Credential Manager、WSL。
- macOS：npm/Homebrew/官方安装器、Keychain、Intel/Apple Silicon。
- Linux：npm/mise/官方脚本、Secret Service 可用与不可用、不同 shell。

## 19. 验收标准映射

| 需求 | 实现落点 | 必测证据 |
| --- | --- | --- |
| 扫描已有 Provider | ImportService + AppAdapter | 多应用扫描结果与来源标签 |
| 导入不改来源 | Adapter 只读 + source fingerprint | 导入前后来源文件 hash 相同 |
| 共享到多个应用 | ProviderBinding | 一个 definition 对应多个 binding |
| 显式启用后写入 | pending/apply 状态机 | 保存 definition 后 live 文件不变 |
| Key 不给前端 | SecretStore + sanitized DTO | IPC 快照中不存在明文和 secret ref |
| 模型只显示实际可用项 | UnifiedModelCatalogService | 按应用和 binding 过滤结果 |
| 应用页管理生命周期 | AppLifecycleCard + LifecycleService | 页面内完成安装、启动、更新、卸载 |
| Codex CLI/Desktop 分开 | component probe | 两个版本和两个启动动作独立 |
| 操作后重新检测 | LifecycleJob verifying | 退出 0、版本未变仍判失败 |
| AI 不执行任意 Shell | Action schema + registry validator | 任意命令和未知 URL 被拒绝 |
| 不显示 Runtime | i18n/UI lint | 新增用户文案自动扫描通过 |

## 20. 完成定义

本补丁只有在以下条件全部满足时才算完成：

1. 新增 Provider 数据不在 SQLite、前端缓存、日志和导出中出现明文 API Key；
2. 从任一支持应用导入后，来源文件保持不变；
3. Provider 修改不会静默写入目标应用，用户可以预览并显式应用；
4. 应用失败有逐目标结果、可验证回滚或明确的人工恢复提示；
5. 应用页可管理对应 CLI/Desktop，且所有成功状态来自后置探测；
6. Codex 助手只能触发登记动作，不能把自然语言转换成任意 Shell 直接执行；
7. Windows、macOS、Linux 的核心路径通过测试，旧 v18 数据可无损迁移；
8. 所有新增用户界面不出现 `Runtime`，新手流程不要求理解 Provider/Binding 的内部实现。
