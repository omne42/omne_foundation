# Changelog

## Unreleased

- Remove the dead default pinned-client cache/expiry branch and keep only per-key build locking plus DNS re-resolution, so the public-IP pinned path now matches its actual always-rebuild behavior without carrying misleading cache state.
- Replace process-global pinned-client cache/build-lock/DNS state with explicit per-`HttpClientProfile` shared state, and keep `select_http_client_with_options(...)` isolated so unrelated callers no longer contend through hidden process-wide HTTP state.
- Rejoin `http-kit` to workspace dependency/lint governance via workspace-managed dependencies and `[lints] workspace = true`.
- Fix `select_http_client_with_options(...)` so `enforce_public_ip=false` still rebuilds the unpinned client from `HttpClientOptions` instead of silently discarding the documented option set in favor of the caller's opaque base client.
- Narrow `allow_localhost` so it only exempts loopback-style hostnames (`localhost`, `localhost.localdomain`, `*.localhost`) instead of also allowing `*.local`, `*.localdomain`, or single-label hosts.
- Require exact IP-literal matches in `allowed_hosts` so malformed suffix entries such as `2.3.4` can no longer allow `1.2.3.4`.
- Re-resolve DNS on every public-IP-pinned client selection instead of reusing a cross-request pinned client cache entry, so DNS failover or rebinding cannot keep routing traffic to a stale address set after connection errors.
- Add the standalone `http-kit` crate with reusable HTTP client, body, URL, and outbound policy helpers.
- Add bounded response streaming support for consumers such as `toolchain-installer`.
- Fix `select_http_client` public-IP pinning so `http://` URLs without an explicit port keep the scheme default instead of being forced to `:443`.
- Add `select_http_client_with_options(...)` and preserve pinned-client builder options/cache partitioning for timeout, connect-timeout, default headers, redirect policy, and proxy policy.
- Reject cross-host redirects while public-IP pinning is active so an initially validated target cannot pivot into an unvalidated redirect destination.
- Bound pinned-client DNS prechecks by the smaller of `timeout` and `connect_timeout`, and normalize public-IP-pinned cache keys to the effective no-proxy behavior.
- Add `HttpClientProfile` so callers can reuse a single explicit `HttpClientOptions` configuration across pinned and unpinned requests without relying on opaque `reqwest::Client` state.
- Remove the timeout-only `select_http_client(...)` convenience API so callers must choose either `HttpClientProfile` or explicit `HttpClientOptions` instead of relying on hidden `reqwest::Client` state during public-IP pinning.
- Add regression coverage for pinned-client redirect policy and proxy bypass so same-host redirects still succeed with preserved default headers, cross-host/scheme/port redirects fail under public-IP pinning, and proxy environment variables cannot intercept pinned requests.
- Add `read_json_body_after_http_success_limited(...)` so callers with known large success payloads can raise the body cap without weakening the crate-wide default.
- Fix public-IP classification for IPv4/IPv6 special-use ranges so `192.0.0.9/32` and `192.0.0.10/32` remain allowed, stale IPv4 anycast blocks are no longer rejected, and `2001::/23` special-use space is treated as non-global except for its documented globally reachable carve-outs.
- Redact URL credentials, paths, and queries from `probe_http_endpoint_detailed(...)` transport error details instead of surfacing raw `reqwest` error strings.
