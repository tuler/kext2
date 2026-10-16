//! Error type and diagnostics.

use std::fmt;

/// An error that makes kext2 exit with status 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// Human-readable message (without the `kext2: ` prefix).
    pub msg: String,
    /// True for command-line usage errors (a hint to `--help` is printed).
    pub usage: bool,
}

impl Error {
    pub fn fatal(msg: impl Into<String>) -> Error {
        Error { msg: msg.into(), usage: false }
    }
    pub fn usage(msg: impl Into<String>) -> Error {
        Error { msg: msg.into(), usage: true }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// `fatal!("fmt", args)` builds a fatal [`Error`].
#[macro_export]
macro_rules! fatal {
    ($($t:tt)*) => { $crate::error::Error::fatal(format!($($t)*)) };
}

/// Attach a context string to an I/O error.
pub fn io_err(ctx: impl fmt::Display, e: std::io::Error) -> Error {
    Error::fatal(format!("{ctx}: {e}"))
}

/// Print a warning to standard error.
pub fn warn(msg: impl fmt::Display) {
    eprintln!("kext2: warning: {msg}");
}

/// Print an informational message to standard error.
pub fn info(msg: impl fmt::Display) {
    eprintln!("kext2: {msg}");
}

/// Render a byte string for messages (lossy UTF-8).
pub fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}
