# Changelog

## [Unreleased]

- localize `omne-fs-primitives` into the `omne_foundation` workspace so foundation crates stop reverse-depending on sibling `omne-runtime` workspace paths for shared filesystem primitives
- call `fs2::FileExt::unlock` with fully qualified syntax so `-D warnings` builds do not fail on the future standard-library name collision lint
