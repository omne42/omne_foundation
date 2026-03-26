# foundation 定义

这个文件回答一个 workspace 级问题：

- 什么是 `foundation`
- 什么不属于 `foundation`
- 应该用什么标准判断一个能力是否应该下沉到这里
- 在 agent-first 语境里，为什么 `harness` 也属于基建

更具体的事实来源请看：

- [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
- [`../crates/README.md`](../crates/README.md)
- [`../../../omne-runtime/README.md`](../../../omne-runtime/README.md)
- [`../../../omne-runtime/docs/workspace-crate-boundaries.md`](../../../omne-runtime/docs/workspace-crate-boundaries.md)
- [`../../../wsl-docs/01-博客/OpenAI/Harness Engineering：Agent 优先时代的 Codex 协作.md`](../../../wsl-docs/01-博客/OpenAI/Harness Engineering：Agent 优先时代的 Codex 协作.md)

## 定义

如果把 `omne_foundation`、`omne-runtime` 以及 agent 的工作环境一起看，`foundation` 指的是：

- 可被多个上层模块、工具或产品复用的基础能力
- 具有稳定领域边界、清晰约束和明确输入输出的底层能力
- 提供原语、协议、运行时能力、统一接线方式，或让 agent 能可靠工作的 harness

换句话说，`foundation` 关心的不是“某个产品功能怎么完成”，而是“多个上层系统反复需要的共性能力，应该如何被稳定地建模、约束、暴露和复用”。

它不是单一仓库名的同义词，而是一组分层基础能力的总称。

在 agent-first 语境里，还需要再补一句：

- `foundation` 不只是“代码库里的公共 crate”
- 它还包括让 agent 可以发现、理解、验证和迭代这些能力的运行环境与反馈系统

也就是说，`foundation` 既包括 software building blocks，也包括 harness。

## Agent-first 视角下的定义补充

参考 `Harness Engineering` 这篇文章，在 agent 优先时代，工程团队的主要工作不再只是手写代码，而是设计环境、表达意图，并构建能让 agent 可靠工作的反馈循环。

因此，`foundation` 在这个语境里应该被理解为四类东西的组合：

- 可复用的底层原语与领域能力
- 让 agent 能直接使用这些能力的标准工具和运行环境
- 让 agent 能理解代码库的知识组织方式
- 让 agent 能验证结果、发现偏差、持续修正的反馈闭环

如果缺少后两类内容，那么即使底层 crate 设计得很好，agent 仍然会因为上下文不可见、验证不可达、约束不可执行而难以稳定工作。

## 不是什么

`foundation` 不等于：

- 单一产品的业务功能
- 只服务一个场景的临时封装
- 上层审批、编排、策略决策本身
- 把不同领域强行揉在一起的“大一统平台”
- 只包含 crate、SDK 或 runtime library 的狭义“底层代码”

如果一个能力离开特定业务上下文就失去意义，它通常不应被视为 `foundation`。

## 判断标准

一个能力是否应该进入 `foundation`，可以先看六件事：

1. 它是否会被多个上层重复依赖，而不是只服务一个调用点。
2. 它解决的是底层共性问题，还是某个业务流程的局部问题。
3. 它能否给出稳定边界：负责什么，不负责什么。
4. 它沉淀下来的是原语、协议、约束、运行时能力，还是业务策略本身。
5. 它是否显著提升 agent 的可理解性、可验证性或可执行性。
6. 它是否能被放进仓库并成为可发现、可维护、可检查的 system of record。

只有当答案更偏向前者时，它才适合进入 `foundation`。

## 边界原则

`omne_foundation` 当前遵循这些原则：

- 一个 crate 只承载一个稳定领域，不把上层应用语义硬塞进 foundation。
- 协议传输、文本语义、资源加载、secret 解析、通知投递分开建模。
- 能由上层应用决定的策略，不下沉到基础 crate。
- 约束优先放在边界处，crate 内部实现保持足够自由。

这些原则的目标不是“把一切抽象成公共库”，而是避免领域污染，保证基础能力可以长期复用。

`omne-runtime` 的边界文档进一步补充了另一条很关键的原则：

- 不创建兜底式 `platform` crate，边界应按能力拆分，而不是按“都是跨平台代码”堆在一起

`Harness Engineering` 进一步提醒我们：

- 文档不只是给人看的说明书，它也是 agent 的运行时上下文
- 约束不应该只停留在口头约定里，而应尽量机械化、可检查
- 任何不在仓库里、不可被 agent 发现的知识，对 agent 来说几乎等于不存在

因此，在 agent-first 工程里，“知识可见性”和“反馈回路”本身就是基建边界的一部分。

## Harness 也是基建

从这个角度看，基建至少包含两大块：

### 1. capability foundation

也就是各种可复用的原语、协议、运行时能力和基础领域能力。

例如：

- `omne-fs-primitives`
- `omne-process-primitives`
- `http-kit`
- `i18n-kit`
- `config-kit`
- `text-assets-kit`
- `i18n-runtime-kit`
- `prompt-kit`
- `secret-kit`
- `mcp-kit`
- `notify-kit`

### 2. harness foundation

也就是让 agent 能稳定使用这些能力的环境、知识和反馈闭环。

典型内容包括：

- 短而稳定的入口文档，例如 `AGENTS.md`、`ARCHITECTURE.md`、`docs/README.md`
- repository-local 的知识系统，例如领域说明、设计记录、plans、references
- 可直接调用的本地工具、脚本、skills、CLI 和工作流
- 测试、lint、结构校验、评审与修复循环
- UI、日志、指标、trace 等可被 agent 直接消费的验证与观测能力

这里尤其要注意：

- `AGENTS.md` 应该是地图，不应该退化成巨大的百科全书
- 真正的事实来源应拆到更窄、更稳定、可维护的仓库内文档中

如果没有 harness foundation，能力本身往往存在，但 agent 很难稳定地把它们转化为可靠产出。

## 更完整的分层视角

如果把 `omne-runtime`、`omne_foundation` 以及 agent harness 一起看，当前更完整的 `foundation` 分层至少有四层：

### 1. systems/runtime primitives 层

这层沉淀“更硬、更窄、尽量无策略”的系统原语。

代表 crate：

- `omne-fs-primitives`
- `omne-process-primitives`

这类能力通常负责：

- capability 风格目录访问
- no-follow open、bounded read、advisory lock 之类文件系统原语
- 进程树启动、挂接、清理和终止原语

这类能力通常不负责：

- 产品策略
- 权限决策
- CLI 合约
- 业务错误映射

### 2. runtime policy / orchestration 层

这层开始解释策略、组织高层操作，或者承担执行编排边界。

代表 crate：

- `omne-fs`
- `omne-execution-gateway`

这类能力通常负责：

- 文件系统 `SandboxPolicy`、高层文件操作和 CLI
- 命令执行边界、隔离级别校验、sandbox 编排和审计

这类能力已经属于基础运行时能力，但不再是“纯原语”。它们开始承载策略解释和运行时编排。

### 3. reusable foundation kits 层

这层是 `omne_foundation` 当前更主要覆盖的部分，面向更稳定的通用基础领域提供 `kit`。

代表领域包括：

- 结构化文本与跨边界表示
- i18n
- 通用文本资源与领域 runtime adapter
- secret 规范与解析
- HTTP client / body / URL / outbound policy
- transport / session
- notify

这一层通常建立在更底层 runtime 原语之上，但目标不是复刻系统能力，而是提供面向应用复用的通用基础能力。

### 4. agent harness / knowledge / feedback 层

这一层不只是“文档附件”，而是 agent-first 工程里的操作系统。

它通常包括：

- 入口地图
- 渐进式披露的知识库
- 计划与决策记录
- 机械化约束和质量闸门
- 自测、自审、观测和修复回路

这一层决定的不是“代码能不能编译”，而是“agent 能不能持续做对事情”。

## 当前 `omne_foundation` 覆盖的基建类型

如果只看 `omne_foundation` workspace，本仓库当前主要覆盖五类 `foundation` 能力：

### 1. 结构化文本语义层

- `structured-text-kit`
- `structured-text-protocol`
- `i18n-kit`

这一层解决的是：

- 用户可见结构化文本如何建模
- 结构化文本如何跨语言、跨边界表示
- 结构化文本如何按 locale 渲染为最终文本

这一层刻意不使用过于宽泛的 “message” 概念，避免和 IM 消息、内部通信消息、事件总线消息混淆。

### 2. 配置与运行时输入层

- `config-kit`
- `text-assets-kit`
- `i18n-runtime-kit`
- `prompt-kit`
- `secret-kit`

这一层解决的是：

- 通用配置文件如何被安全读取、识别格式、层叠与解释
- 通用文本资源如何安全 bootstrap、落盘、回滚、扫描和读取
- i18n catalog 如何从 runtime 目录加载并以 lazy/global handle 形式暴露
- prompt 文本目录如何 bootstrap 并以惰性句柄暴露
- secret 如何通过统一规范被解析、读取和安全持有

它关心的是“如何安全拿到输入并稳定解释输入”，而不是输入内容本身的业务语义。

需要单独强调的是：

- `prompt-kit` 当前只覆盖 prompt 目录型 runtime adapter 这一窄边界。
- 更高层的 prompt bundle identity 与 agent instruction composition，还没有形成统一共享 crate。
- 这部分边界判断见 [`prompt领域定位.md`](./prompt领域定位.md)。

### 3. HTTP foundation 层

- `http-kit`
- `github-kit`

这一层解决的是：

- 共享 HTTP client 如何构建和选择
- 响应体如何有界读取、预览和收口错误
- URL 如何校验、脱敏和约束
- untrusted outbound 目标如何做 IP/DNS/allowlist 校验
- GitHub API 如何以纯 client 方式获取 release metadata

其中：

- `http-kit` 负责通用 transport foundation
- `github-kit` 负责纯 GitHub API client 能力

这层不负责下载来源策略或安装器特有的 asset 选择 / 安装语义。

### 4. 传输与会话层

- `mcp-jsonrpc`
- `mcp-kit`

这一层解决的是：

- 如何建立 transport
- 如何管理连接、初始化生命周期和会话
- 如何提供默认安全边界和配置加载能力

它负责连接和会话能力，不直接定义上层业务工作流。

### 5. 通知层

- `notify-kit`

这一层解决的是：

- 如何把统一事件模型路由到不同通知渠道
- 如何在并发发送、超时和错误聚合之间取得稳定默认行为

它复用共享 HTTP 与日志文本能力，但不负责业务事件生成，也不承担可靠消息系统的语义。

## `omne-runtime` 在整体中的位置

`omne-runtime` 不是 `omne_foundation` 的重复实现，而是更靠近宿主机和运行时边界的一层基础设施。

从它自己的 workspace 文档可以看到，这个仓库有意拆成“两类低层复用 crate”和“两类高层策略/编排 crate”：

低层复用 crate：

- `omne-fs-primitives`
- `omne-process-primitives`

高层策略/编排 crate：

- `omne-fs`
- `omne-execution-gateway`

这说明更完整地看，`foundation` 不是只有 `kit`，还包括：

- 贴近系统边界的 primitives
- 面向 runtime 的策略和编排能力
- 面向通用基础领域的 reusable kits
- 让 agent 能稳定理解和操作这些能力的 harness

从依赖关系上看，`omne_foundation` 也已经直接依赖 `omne-fs-primitives` 和 `omne-process-primitives`，说明这两层不是平行无关，而是存在明确上下游关系。

## 与上层应用的关系

可以把 `foundation` 理解成“上层应用的受约束材料层”：

- 上层应用负责业务目标和产品行为
- `omne_foundation` 负责通用基础领域能力
- `omne-runtime` 负责更贴近系统边界的 runtime 原语与运行时编排能力
- harness 层负责把知识、约束、验证和反馈循环组织成 agent 可直接使用的系统

因此，`foundation` 的价值不在于替代上层，而在于降低重复建设、减少边界混乱，并让不同上层在相同原语和相同反馈系统上协作。

## 一句话总结

`foundation` 不是“把通用代码都堆在一起”，而是把那些跨业务复用、边界稳定、约束明确、并且能被 agent 稳定理解与验证的底层能力，按层次和领域拆开沉淀下来：底层是 systems/runtime primitives，中间是 runtime policy 与 orchestration，更上层是 reusable foundation kits，再往外是一层 agent harness、知识组织和反馈循环。
