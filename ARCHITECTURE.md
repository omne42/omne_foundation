# omne_foundation Architecture

`omne_foundation` 是一个 Rust workspace，目标不是提供单一产品，而是沉淀可复用的基础能力 crate。

这个文件只回答顶层问题：

- workspace 分成哪些领域层
- crate 之间的主要依赖方向是什么
- 应该去哪里找更具体的事实来源

详细的 crate 说明已经拆到 [`docs/crates/README.md`](./docs/crates/README.md) 和各 crate 自己的 `README.md`。

## 读图规则

下面的箭头统一表示：

```text
A -> B   表示 A 依赖 B
```

## 顶层分层

### 1. 跨仓库策略契约层

- `policy-meta`

这一层处理“跨仓库共享的策略字段和契约到底是什么”：

- canonical policy field names 与枚举语义
- JSON Schema / TypeScript 绑定
- baseline profile artifacts

这里刻意只沉淀 contract，不实现决策引擎：

- `policy-meta` 不负责审批、sandbox 或命令执行逻辑。
- 它的目的，是让多个仓库共享同一份稳定 policy vocabulary，而不是重复发明各自的字段集合。

### 2. 文本与观测语义层

- `structured-text-kit`
- `structured-text-protocol`
- `error-kit`
- `error-protocol`
- `log-kit`
- `i18n-kit`

这一层处理“用户可见结构化文本是什么，以及如何跨边界表示它”：

- `structured-text-kit` 定义 `StructuredText` / `CatalogText`
- `structured-text-protocol` 把结构化文本映射到 JSON Schema / TypeScript DTO
- `log-kit` 建模日志文本与日志级别
- `i18n-kit` 按 locale/catalog/template 把结构化文本渲染成最终文本

这里有一个需要显式说明的边界选择：

- 这里刻意不用更泛化的 “message” 概念。
- `structured-text-kit` 只建模“catalog-backed 或 freeform 的用户可见结构化文本”。
- 它不是 IM 消息、进程间通信消息、事件总线消息，也不是通用消息系统。

### 3. 配置与运行时输入层

- `config-kit`
- `text-assets-kit`
- `i18n-runtime-kit`
- `prompt-kit`
- `secret-kit`

这一层处理“配置与运行时输入如何被安全拿到、解析和组织”：

- 配置文件如何被安全读取、识别格式、层叠与解释
- 通用文本资源如何安全地 bootstrap、落盘、回滚、扫描和读取
- i18n catalog 如何从 runtime 目录加载、重载并暴露 lazy/global handle
- prompt 文本目录如何 bootstrap 并以惰性句柄对外提供
- secret 如何通过统一 `secret://` 规范解析

这里有一个当前需要明确的边界：

- `prompt-kit` 目前只承接 prompt 目录 bootstrap / lazy access 这一窄适配层。
- 更高层的 prompt bundle identity 与 agent instruction composition 尚未形成共享 crate。
- 相关判断见 [`docs/定义/prompt领域定位.md`](./docs/定义/prompt领域定位.md)。

### 4. HTTP foundation 层

- `http-kit`
- `github-kit`

这一层处理“如何以共享方式构建和约束 HTTP 出站能力”：

- HTTP client 构建与选择
- 响应体有界读取、preview 与错误收口
- URL 校验与脱敏
- untrusted outbound policy、IP 分类与 DNS 后校验
- GitHub API 请求头、release metadata DTO 与 latest release 获取

### 5. 传输与会话层

- `mcp-jsonrpc`
- `mcp-kit`

这一层处理“如何连接协议端点并管理会话”：

- JSON-RPC transport
- MCP config / initialize / manager / session / security model

### 6. 通知层

- `notify-kit`

这一层处理“如何把统一事件投递到外部通知渠道”：

- 渠道路由、并发发送、超时和错误聚合
- 共享复用 `http-kit` 的 HTTP 能力和 `log-kit` 的文本日志原语

### 7. 音频与语音领域层

- `audio-input-kit`
- `audio-media-kit`
- `model-assets-kit`
- `speech-transcription-kit`
- `speech-whisper-kit`
- `text-postprocess-kit`

这一层处理“音频输入、模型资产、语音转写和文本后处理边界如何共享表达”：

- 音频输入 backend / device / config
- capture session 状态、事件和错误分类
- 音频资产、目标媒体格式、前处理预算和 provenance
- 模型 manifest / source / capability / install status
- 音频输入引用
- 转写 provider/model 选择
- 转写 provider/model 能力和错误分类
- 转写选项
- 文本结果与时间片段
- 转写 job 状态
- 本地 Whisper Rust 运行时适配
- 文本后处理来源、模式、结果、provenance 和错误分类

这里刻意不放具体音频采集实现、通用音频转码实现、模型下载实现、LLM provider adapter 或 HTTP provider 调用。那些能力应分别进入 adapter、`audio-media-kit` adapter、`ditto-llm`、`http-kit` provider adapter、`secret-kit` 和 `omne-runtime` 原语边界。本地 Whisper 的 Rust API 适配先由 `speech-whisper-kit` 承接，兼容外部命令生命周期仍属于 `omne-runtime` 原语边界。

### 8. 桌面输入与文本交付领域层

- `desktop-input-kit`

这一层处理“桌面输入触发和转写文本交付边界如何共享表达”：

- 输入触发来源与事件
- 语音唤醒设置、检测引擎选择、探测/听写预算与唤醒事件
- 文本交付目标、请求和结果
- 桌面权限需求
- 快捷键冲突、麦克风不可用、唤醒词不可用、剪贴板不可用、辅助功能权限缺失和输入模拟失败等错误分类

这里刻意不放 Tauri plugin 接线、系统托盘实现、全局快捷键注册、真实 wake word / keyword spotting 引擎、剪贴板读写、直接输入注入或平台权限引导 UI。那些能力应留在桌面产品层或后续 adapter crate。

## 主要依赖方向

当前 workspace 内部可总结成下面这张简图：

```text
policy-meta            -> (no internal foundation deps)

error-kit              -> structured-text-kit
error-protocol         -> error-kit
error-protocol         -> structured-text-kit
error-protocol         -> structured-text-protocol
structured-text-protocol -> structured-text-kit
log-kit                -> structured-text-kit
i18n-kit              -> structured-text-kit
secret-kit            -> error-kit
secret-kit            -> structured-text-kit

config-kit            -> (no internal foundation deps)
text-assets-kit      -> (no internal foundation deps)
i18n-runtime-kit     -> structured-text-kit
i18n-runtime-kit     -> text-assets-kit
i18n-runtime-kit     -> i18n-kit
prompt-kit           -> text-assets-kit

github-kit           -> http-kit
mcp-jsonrpc          -> error-kit
mcp-jsonrpc          -> structured-text-kit
mcp-jsonrpc           -> http-kit
mcp-kit               -> config-kit
mcp-kit               -> error-kit
mcp-kit               -> http-kit
mcp-kit              -> structured-text-kit
mcp-kit              -> mcp-jsonrpc

notify-kit           -> github-kit
notify-kit           -> http-kit
notify-kit           -> log-kit
notify-kit           -> secret-kit
notify-kit           -> structured-text-kit

audio-input-kit     -> (no internal foundation deps)
audio-media-kit     -> (no internal foundation deps)
desktop-input-kit   -> (no internal foundation deps)
model-assets-kit    -> (no internal foundation deps)
speech-transcription-kit -> audio-media-kit
speech-transcription-kit -> model-assets-kit
speech-whisper-kit  -> (no internal foundation deps)
text-postprocess-kit -> (no internal foundation deps)
```

补充说明：

- `policy-meta` 当前不依赖其他 foundation crate，主要为 `omne-agent`、`omne-runtime` 等外部 workspace 提供共享 contract。
- `error-kit` / `error-protocol` 承接稳定错误语义与跨边界表示；它们属于文本/语义侧基建，不是 transport 或应用编排层。
- `config-kit` 只承接通用配置边界：格式识别、有界读取、路径 canonicalize、strict allowed-format typed parse、layer merge；不拥有产品级 config schema。
- `http-kit` 是通用 HTTP foundation，不承载 GitHub API schema、镜像 / 网关候选策略或其他上层产品语义。
- `github-kit` 建立在 `http-kit` 之上，只负责纯 GitHub API client 能力；它不拥有来源优先级、资产选择或安装编排。
- `text-assets-kit` 刻意不依赖 `i18n-kit`，保持通用文本资源/runtime fs adapter 边界。
- `i18n-runtime-kit` 建立在 `text-assets-kit`、`i18n-kit` 和 `structured-text-kit` 之上，承接目录型 i18n adapter 与 lazy/global handle。
- `prompt-kit` 建立在 `text-assets-kit` 之上，当前只承接 prompt 目录 bootstrap 与惰性访问这一窄适配层，不是 prompt 模板、版本和缓存的统一抽象。
- `mcp-jsonrpc` 与 `mcp-kit` 共享 `http-kit`，并依赖 `error-kit` / `structured-text-kit` 提供稳定错误与文本语义，而不是各自重复实现这些基础表示。
- `notify-kit` 依赖 `http-kit`、`github-kit`、`log-kit`、`secret-kit` 和 `structured-text-kit`，但通知域语义仍独立于 MCP 和 i18n。
- `audio-input-kit` 当前不依赖其他 foundation crate；它只先稳定音频输入 DTO、session 事件和错误语义。
- `audio-media-kit` 当前不依赖其他 foundation crate；它只先稳定音频资产、媒体格式、前处理预算和 provenance 语义。
- `desktop-input-kit` 当前不依赖其他 foundation crate；它只先稳定桌面触发、语音唤醒、文本交付、权限和错误语义。
- `model-assets-kit` 当前不依赖其他 foundation crate；它只先稳定模型 manifest、来源、能力、安装状态和本地引用语义。
- `speech-transcription-kit` 建立在 `audio-media-kit` 的音频资产引用之上，避免转写 job 复制一套资产边界；它也只读复用 `model-assets-kit` 的本地 Whisper catalog 来生成 provider/model descriptor，不拥有模型安装、下载或执行边界。
- `speech-whisper-kit` 当前不依赖其他 foundation crate；它只先稳定 `whisper-rs` 运行时适配、PCM WAV 校验和样本转换。
- `text-postprocess-kit` 当前不依赖其他 foundation crate；它只先稳定后处理请求、结果、状态、provenance 和错误语义，LLM provider 适配继续属于 `ditto-llm`。
- `mcp-jsonrpc` 是 transport 层，`mcp-kit` 在其上增加 MCP 语义和配置管理。
- `i18n-kit` 依赖的是结构化文本原语；`secret-kit` 额外复用 `error-kit` 以暴露稳定错误语义。
- 这张 workspace 内部依赖图现在由 `scripts/check-workspace.sh dependency-direction` 做机械检查；`local` / `ci` 也会先跑它，避免边界只留在文档里。

## 边界原则

这个 workspace 目前遵循几条简单边界原则：

- 一个 crate 只承载一个稳定领域，不把上层应用语义硬塞进 foundation。
- 配置边界、协议传输、结构化文本语义、资源加载、secret 解析、通知投递分开建模。
- 能由上层应用决定的策略，不下沉到基础 crate。
- 约束优先放在边界处，crate 内部实现保持足够自由。

## 记录系统

workspace 级文档现在按“渐进式披露”组织：

- [`AGENTS.md`](./AGENTS.md)
  - 根入口地图，先看这里
- [`docs/README.md`](./docs/README.md)
  - 文档地图，先看这里
- `docs/规范/<topic>.md`
  - workspace 级版本、兼容、发布等治理规则
- [`docs/crates/README.md`](./docs/crates/README.md)
  - crate 索引
- `crates/<crate>/README.md`
  - 每个 crate 的领域、边界、范围、结构设计
- `crates/mcp-kit/docs/`
  - `mcp-kit` 的详细专题文档
- `crates/notify-kit/docs/`
  - `notify-kit` 的详细专题文档

## 文档维护约束

为了避免文档重新退化成“一个巨大的总览文件”，根级文档按下面的规则维护：

- `AGENTS.md` 只保留根入口和硬边界，不承载细节。
- `ARCHITECTURE.md` 只保留 workspace 级地图，不堆实现细节。
- `docs/README.md` 只做入口导航，不重复 crate 细节。
- 版本、兼容、发布等规则写入 `docs/规范/<topic>.md`。
- crate 事实写入对应的 `crates/<crate>/README.md`。
- crate 专题细节优先放到 crate 自己的 `docs/`。
