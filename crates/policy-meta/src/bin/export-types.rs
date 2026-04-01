#[path = "shared/artifacts.rs"]
mod artifacts;
#[path = "shared/cli.rs"]
mod cli;

use std::path::PathBuf;

use artifacts::{check_typescript_bindings, write_typescript_bindings};
use cli::{CliError, next_path_arg};

fn main() -> Result<(), CliError> {
    let mut check = false;
    let mut output_dir = default_output_dir();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--output-dir" => {
                output_dir = next_path_arg("--output-dir", args.next())?;
            }
            other => {
                return Err(CliError::UnknownArgument {
                    arg: other.to_string(),
                });
            }
        }
    }

    if check {
        check_typescript_bindings(&output_dir)?;
        println!("typescript bindings are in sync");
    } else {
        write_typescript_bindings(&output_dir)?;
        println!("wrote typescript bindings to {}", output_dir.display());
    }

    Ok(())
}

fn default_output_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bindings")
}
