use std::io::Write;
#[cfg(not(feature = "sound-command"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "sound-command")]
use tokio::process::Command;

use crate::Event;
use crate::event::Severity;
use crate::sinks::{BoxFuture, Sink};

#[cfg(not(feature = "sound-command"))]
static WARNED_SOUND_COMMAND_DISABLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct SoundConfig {
    pub command_argv: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct SoundSink {
    command_argv: Option<Vec<String>>,
}

impl SoundSink {
    pub fn new(config: SoundConfig) -> Self {
        Self {
            command_argv: config.command_argv,
        }
    }

    fn bell_count(severity: Severity) -> usize {
        match severity {
            Severity::Error => 2,
            Severity::Warning => 1,
            Severity::Info | Severity::Success => 1,
        }
    }

    fn send_terminal_bell(event: &Event) -> crate::Result<()> {
        let bell = "\u{0007}";
        let count = Self::bell_count(event.severity);
        let mut stderr = std::io::stderr().lock();
        for _ in 0..count {
            stderr.write_all(bell.as_bytes())?;
        }
        stderr.flush()?;
        Ok(())
    }

    #[cfg(feature = "sound-command")]
    async fn send_command(command_argv: &[String]) -> crate::Result<()> {
        let (program, args) = command_argv
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("sound command argv is empty"))?;

        if program.trim().is_empty() {
            return Err(anyhow::anyhow!("sound command program is empty").into());
        }

        let mut child = Command::new(program)
            .args(args)
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| anyhow::anyhow!("spawn sound command {program}: {err}"))?;

        let status = child
            .wait()
            .await
            .map_err(|err| anyhow::anyhow!("wait sound command {program}: {err}"))?;

        if !status.success() {
            tracing::warn!(
                sink = "sound",
                program = %program,
                status = ?status,
                "sound command exited non-zero"
            );
        }
        Ok(())
    }
}

impl Sink for SoundSink {
    fn name(&self) -> &'static str {
        "sound"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            if let Some(_argv) = self.command_argv.as_deref() {
                #[cfg(feature = "sound-command")]
                {
                    Self::send_command(_argv).await?;
                    return Ok(());
                }

                #[cfg(not(feature = "sound-command"))]
                {
                    if !WARNED_SOUND_COMMAND_DISABLED.swap(true, Ordering::Relaxed) {
                        tracing::warn!(
                            sink = "sound",
                            "sound command_argv configured but feature \"sound-command\" is disabled; falling back to terminal bell"
                        );
                    }
                    Self::send_terminal_bell(event)?;
                    return Ok(());
                }
            }

            Self::send_terminal_bell(event)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "sound-command")]
    use super::*;

    #[cfg(feature = "sound-command")]
    #[test]
    fn send_command_rejects_empty_argv() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let err = SoundSink::send_command(&[])
                .await
                .expect_err("expected error");
            assert!(err.to_string().contains("argv is empty"), "{err:#}");
        });
    }

    #[cfg(feature = "sound-command")]
    #[test]
    fn send_command_rejects_empty_program() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let err = SoundSink::send_command(&[String::from("  ")])
                .await
                .expect_err("expected error");
            assert!(err.to_string().contains("program is empty"), "{err:#}");
        });
    }
}
