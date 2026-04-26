# http-auth-kit

源码入口：[`src/lib.rs`](./src/lib.rs)

## 领域

`http-auth-kit` 负责可跨仓库复用的 HTTP 鉴权构件。

它沉淀的是 HTTP request auth 装配、OAuth client-credentials token 获取、token/header 表示，以及 AWS SigV4 签名这类稳定协议能力。

## 边界

负责：

- OAuth client-credentials 请求参数构造和 token response 解析
- OAuth authorization header value 生成
- 已解析 token/value 到 header 或 query param 的安全装配
- AWS SigV4 timestamp、canonical request、signature 和 required headers

不负责：

- 产品级 provider auth schema
- 环境变量 key 选择
- provider runtime / transport quirks
- refresh、cache、rotation 策略

## 范围

覆盖：

- `OAuthClientCredentials`
- `OAuthToken`
- `HttpHeaderAuth`
- `HttpQueryParamAuth`
- `HttpRequestAuth`
- `HttpRequestAuthPlan`
- `SigV4Signer`
- `SigV4Timestamp`
- `SigV4Headers`

不覆盖：

- OpenID discovery
- OAuth authorization-code flow
- cloud provider credential chain
