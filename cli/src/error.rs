//! The CLI error type: a user-facing message plus the process exit code. Every
//! lower-layer error converts into one, so command bodies can use `?`.

use std::fmt;

/// A CLI-level failure: the message shown to the user and the exit code.
#[derive(Debug)]
pub struct CliError {
    /// The message written to stderr.
    pub message: String,
    /// The process exit code.
    pub code: i32,
}

impl CliError {
    /// An error exiting with code 1.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 1,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

macro_rules! from_error {
    ($($ty:path),* $(,)?) => {
        $(
            impl From<$ty> for CliError {
                fn from(error: $ty) -> Self {
                    CliError::new(error.to_string())
                }
            }
        )*
    };
}

from_error!(
    runtime::boot::BootError,
    runtime::facade::KernelError,
    runtime::adapters::RuntimeError,
    kernel::install::InstallError,
    kernel::removal::RemovalError,
    std::io::Error,
);
