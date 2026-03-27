# Changelog

## Unreleased

- add `github-kit` for pure GitHub release API client access
- redact fallback release-fetch error targets so api-base credentials, paths, and queries never leak into aggregated diagnostics
- add regression coverage for the parse-failure fallback branch so malformed GitHub API bases are also redacted before aggregation
- raise the GitHub latest-release success-body cap to a bounded crate-local limit so large asset lists are accepted without weakening the default `http-kit` JSON cap for unrelated callers
