# Changelog

## Unreleased

- reject query-bearing or fragment-bearing custom bearer-token API bases so callers cannot smuggle extra URL state into authenticated GitHub requests
- clarify the fail-closed bearer-token latest-release contract so the no-profile API explicitly points callers at the validated DNS/public-IP/http-kit transport path
- add explicit internal dependency version constraints so `cargo package` no longer exports a manifest that drops `http-kit`'s semver edge
- add regression coverage that keeps trusted custom bearer-token API bases fail-closed unless they stay on HTTPS and omit URL credentials
- add `fetch_latest_release_with_profile(...)` and make bearer-token latest-release calls fail closed unless they run through `http-kit::HttpClientProfile`, so GitHub credentials bind to the same DNS/public-IP/pinned-client path as other strict outbound foundations
- make `apply_github_api_headers(...)` validate the target URL before attaching a bearer token so the public helper itself cannot leak credentials to an untrusted custom GitHub API host
- make `apply_github_api_headers(...)` validate the actual `RequestBuilder` target instead of trusting a separate caller-supplied URL, so mismatched helper arguments cannot bypass the bearer-token boundary
- remove the legacy `with_allow_custom_bearer_api_base(true)` bypass so bearer-token requests now require either the canonical GitHub API host or an explicit trusted-host allowlist
- add runtime DNS validation for bearer-token GitHub API requests so trusted custom API bases fail closed on poisoned or unresolvable targets instead of relying on host-string checks alone
- require bearer-token release requests to stay on the canonical GitHub API host unless callers explicitly trust a custom public host allowlist, and keep DNS/private-target validation fail-closed
- refuse to send GitHub bearer tokens to non-HTTPS, localhost, single-label, or private-IP API bases while still allowing tokenless local/mock API bases in tests and controlled callers
- restore the fail-closed bearer-token boundary so custom public GitHub API bases stay blocked by default unless callers explicitly trust them
- add `github-kit` for pure GitHub release API client access
- export shared GitHub API base/url/header helpers so other foundation crates can reuse the canonical request contract instead of rebuilding it locally
- redact fallback release-fetch error targets so api-base credentials, paths, and queries never leak into aggregated diagnostics
- add regression coverage for the parse-failure fallback branch so malformed GitHub API bases are also redacted before aggregation
- raise the GitHub latest-release success-body cap to a bounded crate-local limit so large asset lists are accepted without weakening the default `http-kit` JSON cap for unrelated callers
