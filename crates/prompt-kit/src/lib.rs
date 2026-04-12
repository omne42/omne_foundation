mod prompts;

#[deprecated(
    since = "0.1.0",
    note = "LazyPromptDirectory blocks threads during first initialization; prefer bootstrap_prompt_directory(...) plus PromptDirectoryHandle for runtime use."
)]
#[expect(
    deprecated,
    reason = "crate root intentionally keeps re-exporting LazyPromptDirectory as a deprecated compatibility entrypoint"
)]
pub use prompts::LazyPromptDirectory;
pub use prompts::{
    PromptBootstrapCleanupError, PromptDirectoryError, PromptDirectoryHandle,
    bootstrap_prompt_directory, bootstrap_prompt_directory_with_base,
};
