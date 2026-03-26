# prompt 领域定位

## 目标

这个文件记录一个跨仓库事实：

- 各仓库当前真正需要的 prompt 能力是什么
- 哪些能力值得沉淀为共享基建
- 哪些能力不该因为都叫 “prompt” 就被硬合并成一个抽象

它不是 prompt 写作规范，也不是某个产品仓的实现细节缓存，而是 `omne_foundation` 侧对 prompt 领域边界的长期 system of record。

## 结论先行

当前不应该把 `prompt` 当成一个已经成熟、统一的共享领域。

跨仓库调研后，真实需求只稳定收敛出两类：

### 1. prompt bundle identity

这类需求当前最明显出现在 `captions`：

- 一段稳定的 system instructions
- 一段按业务对象生成的 user text
- 一个结构化输出 schema
- 明确的 prompt/schema version
- 稳定 fingerprint
- 可选的 provider cache key

这里真正要复用的，不是“prompt 文件目录”，而是“prompt bundle 的身份与可追踪性”。

### 2. instruction composition / snapshot

这类需求当前最明显出现在 `omne-agent`：

- base instructions
- user instructions 文件
- project `AGENTS.md`
- mode description
- skills
- 最终快照 hash 与来源可追踪性

这里真正要解决的，不是通用 prompt 模板，而是 agent harness 的指令分层、快照和审计。

## 当前证据

### `captions`

当前使用的是业务内联 prompt bundle：

- `SYSTEM_INSTRUCTIONS`
- `PROMPT_VERSION`
- `SCHEMA_VERSION`
- schema JSON
- fingerprint
- `prompt_cache_key`

这说明 `captions` 的核心需求是：

- prompt 身份稳定
- schema 绑定稳定
- provider cache key 可控

它不需要 prompt 目录 bootstrap，也不需要通用 prompt DSL。

### `omne-agent`

当前运行时主路径是多来源 instructions 组装：

- 默认 instructions
- `OMNE_USER_INSTRUCTIONS_FILE` 或 `~/.omne_data/AGENTS.md`
- thread cwd 下的 `AGENTS.md`
- mode description
- skills 内容

并在运行时生成稳定快照与 hash。

这说明 `omne-agent` 的核心需求是：

- 指令分层顺序稳定
- 来源可解释
- 快照可追踪
- redaction 与 harness 规则可组合

这不是 `prompt-kit` 当前覆盖的目录适配问题。

额外需要明确的一点：

- `omne-agent/.omne_data/prompts/*` 当前不是主运行时路径的 canonical 来源，不应据此把“prompt 目录”误判成已成熟共享领域。

### 其他仓库

当前调研范围内：

- `ditto-llm`
- `db-vfs`
- `omne-runtime`
- `omne-project-init`
- `policy-meta-spec`
- `toolchain-edge-gateway`
- `toolchain-installer`

都没有出现足够明确的、稳定的 prompt 资产 runtime 复用需求。

因此，现阶段没有证据支持继续扩张一个“通用 prompt 平台”。

## 当前边界建议

### 应保留在 `text-assets-kit` / `prompt-kit` 的

- 文本资源目录 bootstrap
- 目录快照加载
- lazy 访问句柄
- 初始化失败与回滚包装

这是一层窄 runtime adapter。

### 应保留在业务仓或 harness 的

- `captions` 的业务 prompt 文案、schema 与缓存策略
- `omne-agent` 的 instructions layering、mode/skill 注入、快照审计
- provider 特有的 prompt cache 行为与 transport 细节

这些能力离开具体产品语境后，语义并不稳定。

### 未来可能进入 `omne_foundation` 的

只有当出现第二个以上稳定消费者时，才值得新增更高层 prompt foundation。

最可能成立的方向不是模板引擎，而是一个更窄的 prompt bundle identity contract，例如：

- instructions text
- user text
- optional structured-output schema attachment
- stable version / fingerprint
- cache-key derivation hook
- source labels / explain metadata

如果未来 `captions` 与 `ditto-llm` 的 L1 或其他业务仓同时稳定需要这一层，再新增 crate 更合理。

## 当前优化建议

### 1. 不扩张 `prompt-kit`

`prompt-kit` 当前本质上是 `text-assets-kit` 之上的薄适配层。

因此当前最合理的动作是：

- 明确它只是目录型 prompt runtime adapter
- 不继续往里面塞模板语义、版本管理、fingerprint、cache key 或业务 schema

### 2. 不建设通用 prompt DSL

当前没有证据支持下列方向：

- 通用 prompt 模板语言
- 通用变量渲染引擎
- 通用 prompt 版本同步系统
- 统一远程 prompt 分发

这些抽象会比实际复用面宽很多。

### 3. 把 agent prompt 视为 harness 问题

`omne-agent` 当前的 prompt 主路径，本质上属于 harness foundation：

- 指令如何分层
- 规则如何进入上下文
- 项目知识如何注入
- 快照如何被解释和审计

这应优先在 agent 仓本地演化，或者未来放进专门的 harness 共享层，而不是强行塞进 `prompt-kit`。

## 触发器

满足下面任一条件时，再重新评估是否新增更高层 prompt crate：

- 出现第二个以上稳定消费者需要 prompt bundle identity
- 至少两个仓库都需要相同的 schema attachment + fingerprint + cache-key contract
- 至少两个 agent/harness 仓库都需要相同的 instructions layering 与 snapshot contract

如果这些条件还没出现，当前最好的优化就是：

- 把 `prompt-kit` 的真实窄边界写清楚
- 避免继续误抽象
- 等真实复用面成熟后再下沉
