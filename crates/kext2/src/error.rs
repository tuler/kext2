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

/// Kind of a diagnostic message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
}

type Sink = Box<dyn FnMut(Level, &str)>;

thread_local! {
    static SINK: std::cell::RefCell<Option<Sink>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with the diagnostics it emits on this thread sent to `sink`
/// instead of standard error (used by library callers such as the Node.js
/// binding). Nested calls restore the outer sink.
pub fn with_messages<T>(sink: impl FnMut(Level, &str) + 'static, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Sink>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0.take();
            SINK.with(|s| *s.borrow_mut() = prev);
        }
    }
    let prev = SINK.with(|s| s.borrow_mut().replace(Box::new(sink)));
    let _restore = Restore(prev);
    f()
}

fn emit(level: Level, msg: &str) {
    let handled = SINK.with(|s| match s.try_borrow_mut() {
        Ok(mut s) => match s.as_mut() {
            Some(sink) => {
                sink(level, msg);
                true
            }
            None => false,
        },
        Err(_) => false,
    });
    if !handled {
        match level {
            Level::Info => eprintln!("kext2: {msg}"),
            Level::Warning => eprintln!("kext2: warning: {msg}"),
        }
    }
}

/// Report a warning (standard error, or the sink of [`with_messages`]).
pub fn warn(msg: impl fmt::Display) {
    emit(Level::Warning, &msg.to_string());
}

/// Report an informational message (standard error, or the sink of
/// [`with_messages`]).
pub fn info(msg: impl fmt::Display) {
    emit(Level::Info, &msg.to_string());
}

/// Render a byte string for messages (lossy UTF-8).
pub fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}
