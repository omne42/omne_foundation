# Changelog

## Unreleased

- route bearer-token release fetches through `http-kit::HttpClientProfile` public-IP pinning so GitHub API requests no longer rely on a one-time static base-url check before sending secrets
- refuse to send GitHub bearer tokens to non-HTTPS, localhost, single-label, or private-IP API bases while still allowing tokenless local/mock API bases in tests and controlled callers
- add `github-kit` for pure GitHub release API client access
- export shared GitHub API base/url/header helpers so other foundation crates can reuse the canonical request contract instead of rebuilding it locally
- redact fallback release-fetch error targets so api-base credentials, paths, and queries never leak into aggregated diagnostics
- add regression coverage for the parse-failure fallback branch so malformed GitHub API bases are also redacted before aggregation
- raise the GitHub latest-release success-body cap to a bounded crate-local limit so large asset lists are accepted without weakening the default `http-kit` JSON cap for unrelated callers
