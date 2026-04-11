use std::{error::Error, path::PathBuf};

use policy_meta::{
    check_schema_dir, check_typescript_bindings, write_schema_dir, write_typescript_bindings,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut check = false;
    let mut schema_dir = default_schema_dir();
    let mut bindings_dir = default_bindings_dir();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--schema-dir" => {
                let Some(path) = args.next() else {
                    return Err("missing path after --schema-dir".into());
                };
                schema_dir = PathBuf::from(path);
            }
            "--bindings-dir" => {
                let Some(path) = args.next() else {
                    return Err("missing path after --bindings-dir".into());
                };
                bindings_dir = PathBuf::from(path);
            }
            other => {
                return Err(format!("unknown argument: {other}").into());
            }
        }
    }

    if check {
        check_schema_dir(&schema_dir)?;
        check_typescript_bindings(&bindings_dir)?;
        println!("all generated artifacts are in sync");
    } else {
        write_schema_dir(&schema_dir)?;
        write_typescript_bindings(&bindings_dir)?;
        println!(
            "wrote generated artifacts to {} and {}",
            schema_dir.display(),
            bindings_dir.display()
        );
    }

    Ok(())
}

fn default_schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema")
}

fn default_bindings_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bindings")
}
