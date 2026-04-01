use std::io::Write;
#[cfg(not(feature = "sound-command"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "sound-command")]
use tokio::process::Command;

use crate::Event;
use crate::event::Severity;
#[cfg(not(feature = "sound-command"))]
use crate::log::warn_sound_command_disabled_fallback;
#[cfg(feature = "sound-command")]
use crate::log::warn_sound_command_exited_non_zero;
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

    fn write_terminal_bells(mut stderr: impl Write, count: usize) -> std::io::Result<()> {
        let bell = "\u{0007}";
        for _ in 0..count {
            stderr.write_all(bell.as_bytes())?;
        }
        stderr.flush()?;
        Ok(())
    }

    async fn send_terminal_bell(event: &Event) -> crate::Result<()> {
        let count = Self::bell_count(event.severity);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            return handle
                .spawn_blocking(move || {
                    let stderr = std::io::stderr();
                    let mut stderr = stderr.lock();
                    Self::write_terminal_bells(&mut stderr, count).map_err(crate::Error::from)
                })
                .await
                .map_err(|err| anyhow::anyhow!("join terminal bell task: {err}"))?;
        }

        let stderr = std::io::stderr();
        let mut stderr = stderr.lock();
        Self::write_terminal_bells(&mut stderr, count)?;
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
            warn_sound_command_exited_non_zero(program, &status.to_string());
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
                        warn_sound_command_disabled_fallback();
                    }
                    Self::send_terminal_bell(event).await?;
                    return Ok(());
                }
            }

            Self::send_terminal_bell(event).await?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_terminal_bells_writes_expected_count() {
        let mut out = Vec::new();
        SoundSink::write_terminal_bells(&mut out, 3).expect("write bells");
        assert_eq!(out, vec![0x07, 0x07, 0x07]);
    }

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
