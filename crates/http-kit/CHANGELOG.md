# Changelog

## Unreleased

- Expose reusable bounded/truncated reqwest body readers with typed limit-vs-transport failures so downstream crates can share one response-body boundary while keeping their own local error mapping.
- Keep SSE limit validation and line-size failures in their original error class instead of recasting them as generic read failures, so downstream adapters can preserve stable error mapping.
- Keep `src/sse.rs` generic by preserving empty `data:` events, removing protocol-specific `[DONE]` termination, and stripping only the single optional space allowed after the SSE field colon.
- Add SSE `data:` stream parsing with bounded line/event limits, plus canonical `send_reqwest_*_after_http_success(...)` helpers so callers can reuse one shared `reqwest + success-check + bounded decode` boundary instead of rewrapping `RequestBuilder` in downstream crates.
- Fix `read_json_body_after_http_success_limited(...)` so non-2xx error summaries honor the caller-provided `max_bytes` limit instead of silently falling back to the crate default cap.
- Return typed transport errors instead of panicking when DNS timeout paths run on a Tokio runtime without the time driver enabled, and keep regression coverage for both untrusted outbound validation and public-IP-pinned client selection.
- Keep untrusted DNS post-validation rejecting always-disallowed targets such as multicast addresses even when `allow_private_ips=true`, so hostnames cannot widen past the hard IP denylist.
- Replace the public `http-kit::Error` wrapper's opaque `anyhow::Error` storage with a structured `kind + message + optional source` boundary, so downstream crates can match stable failure classes without inheriting `anyhow` as the effective public contract.
- Make `allow_localhost=true` consistently accept `localhost` / `*.localhost` DNS answers, including loopback and IPv4 `0/8` host-local results, without also requiring `allow_private_ips=true`; non-localhost hostnames and IP literals still stay fail-closed.
- Keep `allow_private_ips=true` consistent with untrusted host/IP validation by allowing loopback IP literals and `localhost` / `*.localhost` DNS answers, while still rejecting always-disallowed addresses and loopback rebinding from non-localhost hostnames.
- drain bounded successful responses even when `Content-Length` is absent so chunked keep-alive callers can still return connections to the pool
- Break `select_http_client_with_options(...)` by removing the unused `base_client` parameter, so the public API no longer pretends to preserve opaque `reqwest::Client` state that it must rebuild from `HttpClientOptions` anyway.
- Add stable `http_kit::ErrorKind` classification for invalid input, transport, response-body, response-decode, and HTTP-status failures so callers no longer have to pattern-match opaque `anyhow` strings.
- Remove the dead default pinned-client cache/expiry branch and keep only per-key build locking plus DNS re-resolution, so the public-IP pinned path now matches its actual always-rebuild behavior without carrying misleading cache state.
- Replace process-global pinned-client cache/build-lock/DNS state with explicit per-`HttpClientProfile` shared state, and keep `select_http_client_with_options(...)` isolated so unrelated callers no longer contend through hidden process-wide HTTP state.
- Rejoin `http-kit` to workspace dependency/lint governance via workspace-managed dependencies and `[lints] workspace = true`.
- Fix `validate_untrusted_outbound_url_dns(...)` so `allow_private_ips=true` still rejects loopback and always-disallowed resolved addresses instead of short-circuiting DNS validation entirely.
- Narrow `allow_localhost` so it only exempts loopback-style hostnames (`localhost`, `localhost.localdomain`, `*.localhost`) instead of also allowing `*.local`, `*.localdomain`, or single-label hosts.
- Make `parse_and_validate_https_url_basic(...)` share the same localhost/internal-host denylist shape, so direct HTTPS sink validators no longer accept `*.localhost`, `.local`, `.localdomain`, or single-label hosts while outbound-policy rejects them.
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
