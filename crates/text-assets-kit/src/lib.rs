mod bootstrap_lock;
mod data_root;
mod resource_bootstrap;
mod resource_path;
mod secure_fs;
mod text_directory;
mod text_resource;
mod text_tree_scan;

pub use bootstrap_lock::{BootstrapTransactionGuard, lock_bootstrap_transaction};
pub use data_root::{DataRootOptions, DataRootScope, ensure_data_root, resolve_data_root};
pub use resource_bootstrap::{
    BootstrapReport, bootstrap_text_resources, bootstrap_text_resources_with_report,
    rollback_created_resources,
};
pub use resource_path::materialize_resource_root;
pub use secure_fs::{MAX_TEXT_DIRECTORY_TOTAL_BYTES, MAX_TEXT_RESOURCE_BYTES, scan_text_directory};
pub use text_directory::TextDirectory;
pub use text_resource::{ResourceManifest, TextResource};
