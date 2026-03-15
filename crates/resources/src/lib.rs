pub mod data_root;
pub mod text_directory;
pub mod text_resource;

#[cfg(feature = "i18n")]
pub mod i18n;
#[cfg(feature = "prompts")]
pub mod prompts;

pub use data_root::{DataRootOptions, DataRootScope, ensure_data_root, resolve_data_root};
pub use text_directory::{GlobalTextDirectory, TextDirectory};
pub use text_resource::{
    BootstrapReport, ResourceManifest, TextResource, bootstrap_text_resources,
};
