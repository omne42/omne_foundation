# Changelog

## Unreleased

- add runtime DNS validation for bearer-token GitHub API requests so trusted custom API bases fail closed on poisoned or unresolvable targets instead of relying on host-string checks alone
- require bearer-token release requests to stay on the canonical GitHub API host unless callers explicitly trust a custom public host allowlist, and keep DNS/private-target validation fail-closed
- refuse to send GitHub bearer tokens to non-HTTPS, localhost, single-label, or private-IP API bases while still allowing tokenless local/mock API bases in tests and controlled callers
- restore the fail-closed bearer-token boundary so custom public GitHub API bases stay blocked by default unless callers explicitly trust them
- add `github-kit` for pure GitHub release API client access
- export shared GitHub API base/url/header helpers so other foundation crates can reuse the canonical request contract instead of rebuilding it locally
- redact fallback release-fetch error targets so api-base credentials, paths, and queries never leak into aggregated diagnostics
- add regression coverage for the parse-failure fallback branch so malformed GitHub API bases are also redacted before aggregation
- raise the GitHub latest-release success-body cap to a bounded crate-local limit so large asset lists are accepted without weakening the default `http-kit` JSON cap for unrelated callers
