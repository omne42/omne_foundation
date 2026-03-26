use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};

use crate::ErrorRecord;

pub trait CliExitCode: Copy {
    fn as_i32(self) -> i32;
}

#[derive(Debug)]
pub struct CliError<E>
where
    E: CliExitCode,
{
    record: ErrorRecord,
    exit_code: E,
}

pub type CliResult<T, E> = Result<T, CliError<E>>;

impl<E> CliError<E>
where
    E: CliExitCode,
{
    #[must_use]
    pub fn new(record: ErrorRecord, exit_code: E) -> Self {
        Self { record, exit_code }
    }

    #[must_use]
    pub fn record(&self) -> &ErrorRecord {
        &self.record
    }

    #[must_use]
    pub fn exit_code(&self) -> E {
        self.exit_code
    }

    #[must_use]
    pub fn into_record(self) -> ErrorRecord {
        self.record
    }

    #[must_use]
    pub fn into_parts(self) -> (ErrorRecord, E) {
        (self.record, self.exit_code)
    }
}

impl<E> Display for CliError<E>
where
    E: CliExitCode,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self.record.display_text(), f)
    }
}

impl<E> StdError for CliError<E>
where
    E: CliExitCode + fmt::Debug + 'static,
{
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.record)
    }
}
