#[path = "shared/artifacts.rs"]
mod artifacts;

use artifacts::{ExportArtifactsCommand, ExportArtifactsCommandError};

fn main() -> Result<(), ExportArtifactsCommandError> {
    let command = ExportArtifactsCommand::parse_args(std::env::args().skip(1))?;
    let outcome = command.run()?;
    println!("{}", outcome.success_message());
    Ok(())
}
