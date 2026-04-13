mod prompts;

pub use prompts::{
    PromptBootstrapCleanupError, PromptDirectoryError, PromptDirectoryHandle,
    bootstrap_prompt_directory, bootstrap_prompt_directory_with_base,
};

#[doc(hidden)]
pub mod compat {
    #[allow(
        deprecated,
        reason = "compat exposes the deprecated LazyPromptDirectory shim behind an explicit namespace instead of the default crate root API"
    )]
    pub use crate::prompts::LazyPromptDirectory;
}
