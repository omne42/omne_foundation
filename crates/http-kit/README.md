# http-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`http-kit` 负责通用 HTTP transport foundation。

它解决的问题不是“某个产品要访问哪个 API”，而是“如何安全、可复用地构建 HTTP client、校验 URL、读取受限响应体、探测 endpoint，以及校验 untrusted outbound 目标”。

## 边界

负责：

- 通用 HTTP client 构建与选择
- 响应体受限读取、文本 / JSON preview 与错误包装
- URL 校验、脱敏和 path prefix 约束
- untrusted outbound policy
- IP 分类、localhost / private IP 判定和 DNS 解析后校验
- HTTP endpoint 可达性探测

不负责：

- GitHub API schema
- mirror / gateway / canonical 这类下载来源策略
- 工具链资产命名和安装来源选择
- MCP、通知或其他协议的上层语义
- 业务级下载编排

## 范围

覆盖：

- `reqwest::Client` 的共享构建入口
- bounded body read 与 body preview
- HTTPS URL 基础校验与错误脱敏
- untrusted 模式下的 host allowlist、localhost/private IP/DNS 校验
- HTTP probe 与公共 IP 相关判断

不覆盖：

- provider 专属 API 适配
- GitHub release 元数据抓取
- 下载候选去重与来源优先级

## 结构设计

- `src/client.rs`
  - HTTP client 构建、选取与发送辅助
- `src/body.rs`
  - 有界响应体读取、文本 / JSON preview、HTTP 成功状态检查
- `src/url.rs`
  - URL 校验、脱敏与 path prefix 约束
- `src/outbound_policy.rs`
  - `UntrustedOutboundPolicy`、DNS 解析后校验和 host allowlist
- `src/ip.rs`
  - IP 分类辅助
- `src/public_ip.rs`
  - 公网 IP 判定辅助
- `src/http_probe.rs`
  - endpoint 可达性探测
- `src/error.rs`
  - crate 统一错误封装

## 与其他 crate 的关系

- 被 `mcp-jsonrpc`、`mcp-kit`、`notify-kit` 复用，承接共享 HTTP 能力
- `github-kit` 建立在它之上，承接纯 GitHub API client 能力
- 不承载 `toolchain-installer` 的下载来源分类或镜像 / 网关候选策略
