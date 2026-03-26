# Changelog

## Unreleased

- Add the standalone `http-kit` crate with reusable HTTP client, body, URL, and outbound policy helpers.
- Add bounded response streaming support for consumers such as `toolchain-installer`.
- Fix `select_http_client` public-IP pinning so `http://` URLs without an explicit port keep the scheme default instead of being forced to `:443`.
- Add `select_http_client_with_options(...)` and preserve pinned-client builder options/cache partitioning for timeout, connect-timeout, default headers, redirect policy, and proxy policy.
