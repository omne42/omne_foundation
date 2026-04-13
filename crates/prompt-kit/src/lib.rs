mod prompts;

#[doc(hidden)]
#[allow(
    deprecated,
    reason = "crate root intentionally keeps the deprecated LazyPromptDirectory compatibility surface available for downstream callers"
)]
#[deprecated(
    since = "0.1.0",
    note = "LazyPromptDirectory is a blocking compatibility shim. Prefer bootstrap_prompt_directory(...) plus PromptDirectoryHandle for runtime use."
)]
pub use prompts::LazyPromptDirectory;
pub use prompts::{
    PromptBootstrapCleanupError, PromptDirectoryError, PromptDirectoryHandle,
    bootstrap_prompt_directory, bootstrap_prompt_directory_with_base,
};
