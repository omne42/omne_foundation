# github-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`github-kit` 负责 GitHub HTTP API 的窄 client foundation。

它解决的问题不是“某个产品应该优先选哪个 release 资产”，而是“多个调用方如何以一致方式请求 GitHub API，并读取稳定的 release 元数据”。

## 边界

负责：

- GitHub API 请求头默认值与可选 bearer token 注入
- bearer token 默认只发往 canonical GitHub API host；自定义 host 必须显式进入 trusted allowlist，且仍要通过公共出站校验
- bearer-token 请求目标的静态校验与运行时 DNS fail-closed 校验
- `owner/repo` 形式的 repository 标识校验
- latest release endpoint URL 构造
- latest release metadata DTO 与获取
- 多个 GitHub API base 的顺序回退

不负责：

- 环境变量读取
- 产品专属 `User-Agent`
- mirror / gateway / canonical 候选顺序
- release asset 选择策略
- 下载、校验、落盘或安装编排
- issue comment、PR review 之类的其他 GitHub 业务语义

## 范围

覆盖：

- `GitHubApiRequestOptions`
- `GitHubRelease`
- `GitHubReleaseAsset`
- `fetch_latest_release(...)`

不覆盖：

- GitHub GraphQL API
- webhook、OAuth 或设备授权流
- issues / pulls / comments 的高层业务封装

## 结构设计

- `src/client.rs`
  - GitHub API 请求头默认值、request options 与 bearer-token 目标校验
- `src/release.rs`
  - latest release DTO、repository/base URL 归一化与获取
- `src/error.rs`
  - 稳定错误类型

## 与其他 crate 的关系

- 建立在 `http-kit` 之上，复用共享 HTTP request / body / error 收敛能力
- 给 `toolchain-installer` 这类需要 GitHub release metadata 的调用方复用
- 不替代 `http-kit`，也不把 GitHub schema 塞回通用 HTTP transport foundation
