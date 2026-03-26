use std::{error::Error, path::PathBuf};

use policy_meta::{check_typescript_bindings, write_typescript_bindings};

fn main() -> Result<(), Box<dyn Error>> {
    let mut check = false;
    let mut output_dir = default_output_dir();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--output-dir" => {
                let Some(path) = args.next() else {
                    return Err("missing path after --output-dir".into());
                };
                output_dir = PathBuf::from(path);
            }
            other => {
                return Err(format!("unknown argument: {other}").into());
            }
        }
    }

    if check {
        check_bindings_dir(&output_dir)?;
        println!("typescript bindings are in sync");
    } else {
        write_bindings_dir(&output_dir)?;
        println!("wrote typescript bindings to {}", output_dir.display());
    }

    Ok(())
}

fn default_output_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bindings")
}

fn write_bindings_dir(output_dir: &std::path::Path) -> Result<(), Box<dyn Error>> {
    write_typescript_bindings(output_dir)
}

fn check_bindings_dir(output_dir: &std::path::Path) -> Result<(), Box<dyn Error>> {
    check_typescript_bindings(output_dir)
}
