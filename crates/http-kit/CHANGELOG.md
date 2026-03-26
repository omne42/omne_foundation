# Changelog

## Unreleased

- Add the standalone `http-kit` crate with reusable HTTP client, body, URL, and outbound policy helpers.
- Add bounded response streaming support for consumers such as `toolchain-installer`.
- Fix `select_http_client` public-IP pinning so `http://` URLs without an explicit port keep the scheme default instead of being forced to `:443`.
- Add `select_http_client_with_options(...)` and preserve pinned-client builder options/cache partitioning for timeout, connect-timeout, default headers, redirect policy, and proxy policy.
- Reject cross-host redirects while public-IP pinning is active so an initially validated target cannot pivot into an unvalidated redirect destination.
- Bound pinned-client DNS prechecks by the smaller of `timeout` and `connect_timeout`, and normalize public-IP-pinned cache keys to the effective no-proxy behavior.
- Add `HttpClientProfile` so callers can reuse a single explicit `HttpClientOptions` configuration across pinned and unpinned requests without relying on opaque `reqwest::Client` state.
- Add regression coverage for pinned-client redirect policy and proxy bypass so same-host redirects still succeed with preserved default headers, cross-host/scheme/port redirects fail under public-IP pinning, and proxy environment variables cannot intercept pinned requests.
