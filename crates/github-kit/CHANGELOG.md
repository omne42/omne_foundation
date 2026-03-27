# Changelog

## Unreleased

- add `github-kit` for pure GitHub release API client access
- redact fallback release-fetch error targets so api-base credentials, paths, and queries never leak into aggregated diagnostics
- add regression coverage for the parse-failure fallback branch so malformed GitHub API bases are also redacted before aggregation
- raise the bounded JSON-body cap used for latest-release fetches so large but valid GitHub release payloads no longer fail at the shared 16 KiB default
