#![forbid(unsafe_code)]

//! Low-level no-follow filesystem primitives shared by higher-level tooling.
//!
//! This crate owns the descriptor/handle-oriented building blocks that should not be duplicated
//! across policy layers:
//! - root materialization and capability-style directory walking via `cap_std`
//! - no-follow file opens and symlink/reparse-point error classification
//! - bounded UTF-8 file reads with caller-owned limit/error mapping
//! - atomic file writes with staged temp files, validation, and replace semantics

mod advisory_lock;
mod atomic_write;
mod cap_root;
mod path_identity;
mod platform_open;
mod read_limited;

pub use advisory_lock::{AdvisoryLockGuard, lock_advisory_file_in_ambient_root};
pub use atomic_write::{
    AtomicDirectoryError, AtomicDirectoryOptions, AtomicWriteError, AtomicWriteOptions,
    StagedAtomicDirectory, StagedAtomicFile, stage_directory_atomically, stage_file_atomically,
    stage_file_atomically_with_name, write_file_atomically, write_file_atomically_from_reader,
};
pub const DEFAULT_TEXT_FILE_BYTES_LIMIT: usize = 1024 * 1024;
pub const DEFAULT_TEXT_TREE_BYTES_LIMIT: usize = 8 * DEFAULT_TEXT_FILE_BYTES_LIMIT;

pub use cap_root::{
    Dir, File, MissingRootPolicy, RootDir, create_directory_component, create_regular_file_at,
    open_ambient_root, open_directory_component, open_regular_file_at, open_root,
};
pub use path_identity::filesystem_is_case_sensitive;
pub use platform_open::{
    is_symlink_or_reparse_open_error, open_readonly_nofollow, open_regular_readonly_nofollow,
    open_regular_writeonly_nofollow, open_writeonly_nofollow,
};
pub use read_limited::{
    ReadUtf8Error, read_to_end_limited, read_to_end_limited_with_capacity, read_utf8_limited,
};
