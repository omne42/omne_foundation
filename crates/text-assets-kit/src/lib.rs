#[doc(hidden)]
pub mod bootstrap_lock;
mod data_root;
#[doc(hidden)]
pub mod lazy_value;
mod managed_bootstrap;
mod resource_bootstrap;
mod resource_path;
mod secure_fs;
mod text_directory;
mod text_resource;
mod text_tree_scan;

#[deprecated(
    since = "0.1.0",
    note = "BootstrapTransactionGuard and lock_bootstrap_transaction are low-level coordination primitives. Prefer bootstrap_text_resources_then_load(...) or bootstrap_text_resources_with_report(...) at crate boundaries."
)]
pub use bootstrap_lock::{BootstrapTransactionGuard, lock_bootstrap_transaction};
pub use data_root::{
    DataRootOptions, DataRootScope, ensure_data_root, ensure_data_root_with_base,
    resolve_data_root, resolve_data_root_with_base,
};
#[deprecated(
    since = "0.1.0",
    note = "LazyValue is a blocking, thread-oriented compatibility primitive. Prefer eager snapshots or runtime-owned handles at crate boundaries."
)]
pub use lazy_value::{LazyInitError, LazyValue};
pub use managed_bootstrap::{BootstrapLoadError, bootstrap_text_resources_then_load};
pub use resource_bootstrap::{
    BootstrapReport, bootstrap_text_resources, bootstrap_text_resources_with_report,
    rollback_created_resources,
};
pub use resource_path::{materialize_resource_root, materialize_resource_root_with_base};
pub use secure_fs::{MAX_TEXT_DIRECTORY_TOTAL_BYTES, MAX_TEXT_RESOURCE_BYTES, scan_text_directory};
pub use text_directory::TextDirectory;
pub use text_resource::{ResourceManifest, TextResource};
