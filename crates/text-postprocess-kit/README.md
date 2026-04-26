# text-postprocess-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`text-postprocess-kit` 负责文本后处理的共享契约。

它解决的问题不是“怎么调用某个 LLM provider”，而是让语音输入、桌面助手、会议纪要和其他文本工具共享同一套后处理请求、模式、状态、结果、provenance 和错误模型。

## 边界

负责：

- 文本后处理来源
- provider / model 选择快照
- 清理、精简、正式化、自定义等稳定处理模式
- 后处理请求和结果
- 后处理 job 状态
- 后处理错误分类

不负责：

- OpenAI、Anthropic、Google 等 provider adapter
- provider 路由、协议兼容或模型目录
- prompt 管理平台
- secret 解析
- 网络调用
- 产品默认润色策略

## 与其他 crate 的关系

- LLM provider 适配和协议兼容由 `ditto-llm` 负责。
- prompt 资产和模板管理属于 `prompt-kit` 或上层产品。
- 语音转写原始结果属于 `speech-transcription-kit`。
- 产品仓可以先镜像 DTO；稳定后再通过 canonical 依赖消费本 crate。
