#![forbid(unsafe_code)]

//! Tombstone crate.
//!
//! `error-kit` is no longer an active workspace crate. The generic structured
//! user-text primitive moved into `structured-text-kit` and
//! `structured-text-protocol`.

compile_error!(
    "`error-kit` has been retired. Depend on `structured-text-kit` or \
`structured-text-protocol` instead, and build any new error-domain APIs on top of \
those narrower text primitives."
);
