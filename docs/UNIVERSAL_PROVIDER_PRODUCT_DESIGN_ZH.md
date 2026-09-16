# CC Switch 通用 Provider 产品设计（确认基线）

> 状态：已确认，后续产品与实现以本文为准  
> 适用分支：`feat/runtime-lifecycle`  
> UI 参考基线：`7ac2474` 及 CC Switch 原有 Provider 页面  
> 最后确认：2026-09-16  
> 术语：面向用户统一称“通用 Provider”；`ProviderDefinition`、`Binding`、`Projection`、`Provider Center` 只作为内部实现概念。

## 1. 文档效力

本文记录已经确认的通用 Provider 产品方案，目的是防止后续实现重新偏向第二套 Provider 表单、强制分组列表或独立的重型管理中心。

当本文与以下旧文档中涉及通用 Provider 的交互设计冲突时，以本文为准：

- `IMPLEMENTATION_DESIGN_PROVIDER_AND_APP_MANAGEMENT_ZH.md`
- `PATCH_REQUIREMENTS_PROVIDER_AND_APP_MANAGEMENT_ZH.md`

旧文档中有关应用安装、启动、更新、卸载等非 Provider 内容不受本文影响。

## 2. 产品目标

通用 Provider 要解决的问题是：

> 同一个模型服务上游只配置一次，即可添加给多个 Agent 使用；共享信息集中维护，各 Agent 的专属能力继续使用 CC Switch 原来的表单和配置格式。

主要目标：

1. 减少用户在多个 Agent 中重复填写名称、API Key、Base URL、协议和模型等信息。
2. 复用 CC Switch 已有的 Provider 新增、编辑、卡片、排序、启用、测速、用量和本地路由能力。
3. 支持一个通用 Provider 被多个 Agent 绑定，并区分直接兼容、路由兼容和不兼容。
4. 共享字段只维护一次，同时保留每个 Agent 的专属配置。
5. 添加完成后与 CC Switch 原行为一致：只保存 Provider，不自动切换当前 Provider，也不自动启动路由。
6. 用户无需学习 Provider Center、Binding、Projection、Revision 等内部概念。

## 3. 本期范围与非目标

### 3.1 本期范围

- 新建通用 Provider。
- 将通用 Provider 添加到一个或多个 Agent。
- 在现有 Agent 页面展示通用 Provider。
- 普通列表与分类视图。
- 使用各 Agent 原 ProviderForm 补充专属必填配置。
- 编辑通用 Provider 的共享字段与当前 Agent 专属字段。
- 管理通用 Provider 的 Agent 使用范围。
- 从当前 Agent 移除、保留为独立 Provider、彻底全局删除。
- 直接兼容与本地路由兼容判断。
- 安全凭证、变更预览、事务写入、失败回滚和漂移保护。

### 3.2 本期明确不做

- 不改进复制粘贴或跨 Agent 复制功能。
- 不新增一套独立的“通用 Provider 简化表单”。
- 不用通用 Provider 页面替代各 Agent 原 ProviderForm。
- 第一版不提供独立、重型、需要用户学习的 Provider Center 页面。
- 不把 Official、OAuth、Coding Plan 或原生账号强行提升为通用 Provider。
- 添加通用 Provider 后不自动启用、不自动切换、不自动启动本地代理。
- 不默认覆盖发生外部修改或漂移的配置。

## 4. 核心设计原则

### 4.1 CC Switch 原表单是唯一 Provider 配置入口

Claude、Codex、Gemini、OpenCode、OpenClaw、Hermes、Pi、Grok Build、Claude Desktop 等 Agent 继续使用各自原有的：

- `ProviderForm`；
- Provider 预设；
- 字段布局；
- 高级选项；
- 模型映射；
- 原生账号管理；
- 自定义端点；
- 原有校验和默认值。

通用 Provider 只扩展保存范围、目标 Agent、兼容性和共享关系，不重新实现这些表单。

### 4.2 通用 Provider 是共享关系，不是通用配置文件

通用 Provider 不是一份可以原样写进所有 Agent 的 JSON。它由三层组成：

```text
通用 Provider
├─ 共享上游定义
├─ Agent Binding
└─ Agent 专属配置模板 / 投影
```

### 4.3 不改变 CC Switch 原有运行语义

“添加”只表示保存到选中的 Agent Provider 列表中：

- 不修改当前 Provider；
- 不调用 Provider 切换；
- 不修改当前模型；
- 不自动打开代理接管；
- 不停止或重启当前运行中的 Provider。

以后用户在某个 Agent 页面点击“启用”时，继续走 CC Switch 原有启用和路由接管流程。

## 5. 通用 Provider 数据模型

### 5.1 共享上游定义

共享定义保存真正跨 Agent 共用的内容：

- 名称；
- 官网；
- 图标及颜色；
- 备注；
- 上游协议；
- Base URL；
- API Key 或其他凭证引用；
- 上游模型目录；
- 非敏感通用元数据；
- 修订版本。

API Key 不得明文保存在普通数据库字段、前端状态、IPC 响应、日志或绑定模板中。

### 5.2 Agent Binding

Binding 表示某个通用 Provider 已被添加给某个 Agent，至少记录：

- Agent 类型；
- 生成的 Agent Provider ID；
- 连接方式：直接连接或本地路由；
- 是否已添加；
- 共享定义目标版本和已应用版本；
- 当前状态；
- 配置指纹与漂移状态；
- 当前 Agent 的非敏感专属配置模板；
- 最近一次事务与错误摘要。

### 5.3 Agent 专属配置

以下内容不强行放入共享定义，而属于对应 Agent 的 Binding：

- Claude 的模型别名、自定义端点及 Claude 专属选项；
- Codex 的 `wire_api`、模型映射、reasoning 和 User-Agent 等；
- Pi 的 `providerKey`、模型目录和 Pi 专属元数据；
- OpenClaw 的默认模型、fallback 和专属默认值；
- OpenCode、Hermes、Gemini、Grok Build、Claude Desktop 的原生专属字段。

规则：

- 修改共享字段可能影响所有绑定 Agent；
- 修改 Agent 专属字段只影响当前 Binding；
- 不支持隐式、难以解释的共享字段单 Agent override；
- 如果用户要让某个 Agent 的共享字段独立变化，应先“保留为独立 Provider”。

## 6. Agent 首页与 Provider 列表

### 6.1 页面总体结构

沿用 `7ac2474` 的页面信息结构和 CC Switch 原卡片能力：

```text
Agent 启动器 / 应用状态
Provider 列表
```

启动器不是 Provider，不参与 Provider 排序。Provider 根据用户选择使用普通列表或分类视图显示。

### 6.2 普通列表（默认）

普通列表是默认显示方式：

- 所有 Provider 使用原 `ProviderCard`；
- 沿用用户现有顺序；
- 沿用全局拖拽；
- Official、通用、当前 Agent 专用 Provider 可以混合排列；
- 通用 Provider 只增加轻量“通用”标记；
- 需要代理时继续显示“需要路由”；
- 不因通用所有权而禁用整张卡片。

卡片继续保留原有能力，包括适用时的：

- 启用；
- 编辑；
- 复制；
- 测速；
- 用量；
- 终端或 endpoint 操作；
- 删除；
- 排序。

通用 Provider 只对所有权相关操作采用专门语义，例如受管编辑和三种删除处理方式。

### 6.3 分类视图（用户可选）

工具栏提供显示方式切换：

```text
普通列表 | 分类视图
```

分类视图按以下固定结构展示：

1. 官方账号与 Coding Plan；
2. 通用 Provider；
3. 仅当前 Agent。

规则：

- 分类只改变显示，不重写底层 Provider 顺序；
- 切回普通列表后恢复原顺序；
- 分类内允许拖拽排序；
- 不允许通过跨分类拖拽改变 Provider 所有权；
- 搜索时保留分类，空分类隐藏；
- 记住用户的显示方式偏好；
- 各分类继续复用同一套原 Provider 卡片，不创建专用简化卡片。

“分类视图”是显示方式，不应命名为会让用户误以为永久重排数据的“自动整理”。

### 6.4 Official 与原生账号

Official、OAuth、Coding Plan 和原生账号归入“官方账号与 Coding Plan”，其认证语义保持不变：

- `codex-official / OpenAI Official` 语义不变；
- 不接管 `~/.codex/auth.json`；
- 不把原生账号凭证转换成通用 API Key；
- 不允许通过通用 Provider 流程绕过原有账号管理。

## 7. 新建通用 Provider

### 7.1 入口

用户仍从当前 Agent 页面点击原来的“+”，打开当前 Agent 原 AddProviderDialog 和 ProviderForm。

表单底部增加保存范围：

```text
保存为
● 当前 Agent Provider
○ 通用 Provider
```

- “当前 Agent Provider”完全沿用 CC Switch 原提交逻辑；
- “通用 Provider”使用同一份 ProviderForm 草稿进入目标 Agent 选择步骤；
- 对 Official、OAuth、Coding Plan 等不可共享类型隐藏或禁用“通用 Provider”。

### 7.2 目标 Agent 与兼容性预览

选择通用 Provider 后，下一步必须显示当前支持的全部 Agent：

```text
通用 Provider：sunrise
上游协议：OpenAI Chat Completions

添加到以下 Agent：
☑ Claude Code       路由兼容
☑ Claude Desktop    路由兼容
☑ Codex             路由兼容
☑ OpenCode          直接兼容
☑ OpenClaw          直接兼容
☑ Pi                直接兼容
☑ Grok Build        直接兼容
— Gemini             当前不支持
```

每个 Agent 只能处于以下状态之一：

#### 直接兼容

目标 Agent 可以使用原生配置直接访问该上游协议，不需要 CC Switch 协议转换。

#### 路由兼容

目标 Agent 的原生协议与上游协议不同，但 CC Switch 当前已有明确的双向转换路径，包括请求、响应和流式响应。

文案必须明确：

> 可以添加；以后启用此 Provider 时需要 CC Switch 本地路由。

保存时不自动启动路由。

#### 当前不支持

目标 Agent 既不能直接访问，也没有已实现的协议转换路径。该项不可选择，并展示原因。

兼容性不能只比较静态协议字符串，必须结合 CC Switch 真实代理转换能力。

### 7.3 默认选择

- 当前 Agent 默认选中；
- 所有确认兼容的 Agent默认选中，以减少重复操作；
- 用户可以取消不希望添加的目标；
- 不兼容 Agent 不可选；
- 最终确认必须明确列出所有将创建 Provider 的 Agent，以及每个 Agent 的直连或路由方式。

“通用”表示可被多个兼容 Agent 共享，不表示系统可以未经确认静默写入所有 Agent。

### 7.4 目标 Agent 专属字段

系统按以下顺序处理目标 Agent 配置：

1. 从来源 Agent 原表单草稿中提取共享字段；
2. 保存来源 Agent 的专属字段到来源 Binding；
3. 使用目标 Agent adapter 和原默认值生成目标 Provider 草稿；
4. 自动补齐能够安全推导的字段；
5. 仅在缺少真正必填字段时标记“需要完善”；
6. 用户点击“完善配置”时打开目标 Agent 原 ProviderForm，并预填已有内容；
7. 完成后返回目标 Agent 选择页。

不得在通用 Provider 流程中重新实现 Codex、Pi、OpenClaw 等 Agent 的简化表单。

### 7.5 保存与确认

最终确认显示：

- 共享名称、协议、Base URL 和模型摘要；
- 凭证是否已配置，不显示凭证明文；
- 选中的 Agent；
- 各 Agent 的连接方式；
- 是否仍有必填配置未完善；
- 将创建还是更新；
- 漂移或冲突状态。

用户确认后，以一个事务执行：

1. 创建共享定义；
2. 写入安全凭证；
3. 创建选中 Agent 的 Binding；
4. 生成各 Agent 原生 Provider 投影；
5. 写入各 Agent 的 Provider 列表；
6. 校验写入结果；
7. 任意一步失败时回滚本次全部写入。

成功后：

- Provider 出现在所选 Agent 的 Provider 列表中；
- 保持各 Agent 当前 Provider 不变；
- 不自动启用；
- 不自动启动代理；
- 卡片显示“通用”以及必要的“需要路由”标记。

不提供“仅保存到通用库”和“保存并应用”两套多余选择。保存行为直接与 CC Switch 原添加语义对齐。

## 8. 添加已有通用 Provider

当前 Agent 尚未绑定某个通用 Provider 时，从“+”入口提供：

```text
新建 Provider
添加已有通用 Provider
```

“添加已有通用 Provider”只显示选择列表，不提供第二套编辑表单：

```text
sunrise        OpenAI Chat · 路由兼容
NewAPI         OpenAI Chat · 直接兼容
company        Anthropic · 直接兼容
```

选中后：

1. 生成当前 Agent 草稿；
2. 自动补齐能够推导的字段；
3. 缺少必填字段时打开当前 Agent 原 ProviderForm；
4. 显示保存预览；
5. 确认后添加到当前 Agent 列表；
6. 不自动切换或启用。

## 9. 编辑通用 Provider

### 9.1 编辑入口

通用 Provider 卡片的编辑按钮继续打开当前 Agent 原 EditProviderDialog，而不是跳转到独立管理大表单。

表单顶部轻量提示：

```text
通用 Provider · 已添加到 N 个 Agent
```

字段可标识为：

- “共享”：修改后可能影响全部 Binding；
- “仅当前 Agent”：只修改当前 Agent 专属配置。

### 9.2 保存预览

如果只修改 Agent 专属字段，预览明确说明：

```text
本次修改只影响 Claude Code。
```

如果修改共享字段，预览必须列出：

- 修改前后值；
- 受影响的全部 Agent；
- 每个 Agent 更新后的兼容方式；
- 是否从直连变为路由或从路由变为不兼容；
- 是否存在漂移或冲突。

共享字段更新必须显式确认并事务写入。普通 `update_provider` 不得绕过通用 Provider 所有权直接修改投影。

### 9.3 单 Agent 独立修改共享字段

如果用户只想让一个 Agent 使用不同的 Base URL、凭证、协议或共享模型，应先选择“保留为独立 Provider”，再使用原编辑流程修改。

第一版不提供隐式共享字段 override，避免出现同名通用 Provider 实际各 Agent 配置不同但用户无法判断的情况。

## 10. 管理通用 Provider 的 Agent 范围

第一版不提供独立 Provider Center 页面。

用户可以点击卡片上的“通用”标记，或卡片菜单中的“管理使用范围”，打开轻量弹窗或侧边抽屉：

```text
sunrise
OpenAI Chat Completions

使用范围
☑ Claude Code       路由兼容 · 已添加
☑ Claude Desktop    路由兼容 · 已添加
☑ Codex             路由兼容 · 已添加
☑ OpenCode          直接兼容 · 已添加
☐ Pi                直接兼容 · 未添加
— Gemini             当前不支持
```

该界面支持：

- 添加到新的 Agent；
- 从某个 Agent 移除；
- 查看直接兼容、路由兼容或不兼容；
- 查看待更新、失败或配置漂移；
- 打开对应 Agent 的原编辑表单；
- 查看共享字段变更影响范围。

内部仍可使用 Provider Center 领域服务，但普通用户界面不暴露该名称和技术概念。

## 11. 删除与解除共享

通用 Provider 卡片点击垃圾桶后，直接提供三种处理方式。

### 11.1 从当前 Agent 移除（默认）

```text
从 Claude Desktop 移除

删除当前 Agent 中的 Provider 和通用绑定，
不会影响其他 Agent。
```

执行结果：

- 删除当前 Agent 的 Binding；
- 删除当前 Agent 的受管 Provider 投影；
- 不影响其他 Agent；
- 共享定义仍有其他 Binding 时继续保留。

### 11.2 保留为独立 Provider

```text
保留为独立 Provider

解除与通用 Provider 的关联，保留当前配置。
以后修改不会影响其他 Agent。
```

执行结果：

- 保留当前 Agent 的完整原生 Provider；
- 删除当前 Binding；
- 清除 Provider Center 所有权标记；
- 恢复普通 Provider 的编辑、删除和排序语义；
- 凭证由后端安全复制或迁移，不经过前端明文传递。

### 11.3 从所有 Agent 中彻底删除

该选项位于同一弹窗的“危险操作”区域，不要求用户进入独立 Provider Center 页面：

```text
从所有 Agent 中彻底删除

将删除通用 Provider 及其在以下 Agent 中的配置：
- Claude Code
- Claude Desktop
- Codex
- OpenCode
```

规则：

- 使用危险色和单独说明；
- 选择后必须进行第二次确认；
- 二次确认列出所有受影响 Agent；
- 删除全部 Binding、受管投影、共享定义和安全凭证；
- 不删除 Official 账号或不相关的独立 Provider；
- 任一步失败时执行事务回滚。

### 11.4 最后一个 Binding

如果“从当前 Agent 移除”会删除最后一个 Binding，必须明确提示：

```text
这是该通用 Provider 的最后一个 Agent。
移除后，共享定义和安全凭证也会被删除。
```

此时不必重复展示效果相同的全局删除选项。

### 11.5 正在使用与配置漂移

- 如果 Provider 正在任何受影响 Agent 中使用，不得静默切换到其他 Provider；第一版默认阻止删除，并要求用户先切换。
- 如果配置被外部修改，不得静默删除用户修改；默认阻止或允许解除管理并保留本地配置。
- 强制删除如未来提供，必须先展示差异并再次确认。

## 12. 兼容性与本地路由

### 12.1 单一能力来源

通用 Provider 兼容性与 CC Switch 本地代理必须使用同一能力注册表，不得维护两套互相矛盾的列表。

概念接口：

```text
resolveCompatibility(upstreamProtocol, targetAgent)
```

返回：

```text
Direct
Proxy {
  requestTransform,
  responseTransform,
  supportsStreaming,
  requiresTakeover
}
Unsupported {
  reason
}
```

### 12.2 当前已确认的代理能力

当前 CC Switch 至少已有以下协议转换能力：

- Claude/Claude Desktop 客户端 → OpenAI Chat 上游；
- Claude/Claude Desktop 客户端 → OpenAI Responses 上游；
- Claude/Claude Desktop 客户端 → Gemini 上游；
- Codex 客户端 → OpenAI Chat 上游；
- Codex 客户端 → Anthropic 上游。

转换能力必须以代码中实际存在的请求、响应及流式转换实现为准，不得仅根据名称推测。

Gemini 等当前没有相应客户端转换路径的组合应标记为不支持，不承诺“任意协议互转”。

### 12.3 路由生命周期

- 保存路由兼容 Provider 时只记录连接方式和生成 Provider 配置；
- 不自动启动本地代理；
- 用户以后点击“启用”时复用 CC Switch 原本的路由接管逻辑；
- 卡片明确显示“需要路由”；
- 代理不可用时，启用流程给出真实错误，不伪装为 Provider 本身无效。

## 13. 模型管理

模型分为两层。

### 13.1 上游共享模型目录

记录上游实际提供的模型，可来自：

- 当前 Agent 原表单；
- 用户手动添加；
- API 模型发现；
- Provider 预设；
- 只读扫描后显式导入。

### 13.2 Agent 模型映射

各 Agent 如何使用上游模型属于 Binding，例如：

- Codex 模型映射；
- Claude 默认模型或别名；
- Pi 展示名称与模型目录；
- OpenClaw 默认模型和 fallback。

共享模型目录变更时：

- 显示受影响 Agent；
- 不静默覆盖 Agent 专属映射；
- 不能表达的能力在预览中提示，不静默丢弃；
- 只有确认后才同步。

## 14. 状态与卡片反馈

通用 Provider 可显示以下轻量状态，不改变原卡片主体：

- `通用`；
- `需要路由`；
- `待更新`；
- `配置漂移`；
- `应用失败`。

状态说明：

- “通用”表示受共享定义管理；
- “需要路由”表示以后启用时依赖本地代理；
- “待更新”表示共享定义有新版本尚未安全投射；
- “配置漂移”表示目标 Agent 配置被外部修改；
- “应用失败”应提供可理解的错误和重试入口。

catalog 或所有权查询失败时：

- 不猜测 definition ID；
- 不让整张卡片永久失效；
- 保留安全操作；
- 显示状态并提供重试。

## 15. 安全与一致性要求

### 15.1 凭证安全

- API Key 只写入安全存储；
- IPC 和前端 DTO 只返回 `credentialConfigured`、掩码尾部等摘要；
- Binding 模板不得包含明文凭证；
- 日志、错误和事务摘要不得包含明文凭证；
- 不得降级为 SQLite 明文；
- 不得接管 `~/.codex/auth.json`。

### 15.2 预览与确认

以下操作必须先预览再确认：

- 创建跨 Agent 通用 Provider；
- 修改共享字段；
- 增加或移除多个 Agent；
- 彻底全局删除；
- 处理漂移后的覆盖或删除。

预览 token 必须有有效期，并绑定：

- 操作类型；
- 共享定义修订；
- 目标 Agent 集合；
- 非敏感配置摘要。

确认时重新校验，不能使用过期预览执行写入。

### 15.3 事务与回滚

创建、更新、绑定、解绑、保留独立和全局删除均需要：

- 应用级操作锁；
- 乐观并发修订检查；
- 幂等键；
- 写入前快照；
- 写入后指纹校验；
- 失败回滚；
- 可恢复的事务记录。

### 15.4 所有权保护

`guard_projection_mutation` 必须保持有效：

- 普通更新、删除或 remove 命令不得绕过共享所有权直接修改受管投影；
- 通用 Provider 编辑走共享事务；
- “保留为独立 Provider”成功清除所有权后，才恢复原普通修改路径；
- usage、endpoint、sort 等旁路操作按明确能力判断，不得偶然绕过保护。

## 16. 推荐用户文案

面向用户避免以下内部术语：

- Provider Center；
- Definition；
- Binding；
- Projection；
- Revision；
- Transaction。

推荐文案：

- 通用 Provider；
- 当前 Agent Provider；
- 添加到以下 Agent；
- 直接兼容；
- 路由兼容；
- 当前不支持；
- 管理使用范围；
- 从当前 Agent 移除；
- 保留为独立 Provider；
- 从所有 Agent 中彻底删除；
- 待更新；
- 配置已被外部修改。

## 17. 实施顺序

### 阶段 1：恢复正确 UI 基线

- 恢复 `7ac2474` 认可的启动器和 Provider 页面结构；
- 恢复原 AddProviderDialog、EditProviderDialog、ProviderForm 和 ProviderCard 行为；
- 移除独立通用 Provider 大表单对原表单的覆盖；
- 保留已有底层共享定义、Binding、安全凭证、事务和回滚能力。

### 阶段 2：列表与保存范围

- 增加普通列表和分类视图；
- 通用 Provider 增加轻量标记；
- 原添加表单增加“当前 Agent / 通用 Provider”保存范围；
- 保持普通 Provider 提交路径不变。

### 阶段 3：多 Agent 创建

- 建立统一兼容性能力注册表；
- 展示全部 Agent 的直接、路由和不兼容状态；
- 复用目标 Agent 原表单补充必填字段；
- 完成预览、确认、事务保存与回滚；
- 确保保存后不切换和不启动代理。

### 阶段 4：编辑、范围和删除

- 受管投影复用原 EditProviderDialog；
- 增加共享字段影响预览；
- 增加轻量“管理使用范围”；
- 完成当前移除、保留独立和全局删除三种语义。

### 阶段 5：模型与防御性状态

- 完善共享模型目录和 Agent 模型映射；
- 漂移检测、失败恢复和状态展示；
- 覆盖旁路命令和边界测试。

## 18. 验收标准

### 18.1 UI 与原功能

- 默认普通列表保持原顺序和拖拽；
- 可切换到“官方 → 通用 → 仅当前 Agent”的分类视图；
- 分类视图不改写底层顺序；
- 所有 Agent 仍使用自己的原 Add/Edit ProviderForm；
- 原卡片的启用、测速、用量、复制等安全操作没有被通用所有权一刀切移除。

### 18.2 创建

- 用户只填写一次来源 Agent 原表单；
- 选择通用后能看到全部 Agent 的兼容状态；
- 路由兼容与直接兼容明确区分；
- 只有真正缺少必填字段时才打开目标 Agent 原表单；
- 最终确认前无 Provider、Binding 或投影残留；
- 保存后各 Agent 当前 Provider 不变，代理状态不变。

### 18.3 编辑

- 普通 Provider 编辑行为完全不变；
- 通用 Provider 仍使用当前 Agent 原编辑器；
- 共享字段变更显示全部受影响 Agent；
- Agent 专属字段只影响当前 Agent；
- 不允许普通更新命令绕过共享所有权。

### 18.4 删除

- 删除弹窗提供当前移除、保留独立、全局删除；
- 当前移除不影响其他 Agent；
- 保留独立后可走普通编辑和删除；
- 全局删除二次确认并列出所有受影响 Agent；
- 正在使用或配置漂移时不静默丢数据；
- 失败时恢复所有已修改目标。

### 18.5 安全

- 前端、IPC、普通数据库、日志和绑定模板中不存在明文 API Key；
- Official/OAuth/Coding Plan 语义不变；
- 不接管 Codex `auth.json`；
- 所有跨 Agent 写入均有显式确认、修订校验、事务和回滚。

## 19. 最终产品定义

通用 Provider 的最终定义是：

> 一份由用户显式创建和管理的共享模型服务上游。它可以按真实兼容能力添加给多个 Agent；共享名称、凭证、地址、协议和上游模型，各 Agent 继续保留自己的原生配置和模型映射。创建、编辑、启用、删除均最大限度复用 CC Switch 原有页面和行为。

它不是：

- 一份强行适配所有 Agent 的统一 JSON；
- 第二套 Provider 表单；
- 添加后自动切换所有 Agent 的自动化；
- 要求用户理解 Binding 和 Projection 的重型管理后台；
- Official 账号或 OAuth 登录的替代品。
