#[doc(hidden)]
pub mod prompts;

#[deprecated(
    since = "0.1.0",
    note = "LazyPromptDirectory blocks threads during first initialization; prefer bootstrap_prompt_directory(...) plus PromptDirectoryHandle for runtime use."
)]
pub use prompts::LazyPromptDirectory;
pub use prompts::{
    PromptBootstrapCleanupError, PromptDirectoryError, PromptDirectoryHandle,
    bootstrap_prompt_directory,
};
