//! Logging: console output plus optional per-category files.
//!
//! The Node agent used pino with pino-pretty on stdout; the idiomatic Rust
//! equivalent is `tracing` with a `tracing-subscriber` formatter, which is what
//! this is — not a line-for-line port of pino. `LOGLEVEL` and `PLAIN_LOG` stay
//! compatible with the Node agent; `LOG_FORMAT` and `LOG_DIR` are additions.

mod console;
mod files;

use std::path::Path;

use tracing_subscriber::EnvFilter;

pub use files::{is_access, is_agent, is_error, is_sync, ACCESS_TARGET};

/// Shape of the log stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable single line, one per event. The default.
    Pretty,
    /// One JSON object per line, for collectors and log shippers.
    Json,
}

impl LogFormat {
    /// Read a `LOG_FORMAT` value; only `json` selects JSON.
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("json") => LogFormat::Json,
            _ => LogFormat::Pretty,
        }
    }
}

/// Initialise the global tracing subscriber.
///
/// `level` mirrors the Node agent's `LOGLEVEL` (default `info`). `RUST_LOG`
/// still wins when it is set, so a targeted filter is always possible. `plain`
/// (from `PLAIN_LOG`) disables ANSI colouring; JSON output is never coloured.
///
/// When `dir` is given, every event is also appended to one file per category
/// there; the console keeps receiving the whole stream.
pub fn init(level: &str, plain: bool, format: LogFormat, dir: Option<&Path>) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    if let Some(dir) = dir {
        match files::init(filter.clone(), plain, format, dir) {
            Ok(()) => return,
            // A directory we cannot write is worth saying out loud, but it must
            // not stop the agent: fall back to the console alone.
            Err(e) => eprintln!("cannot write logs to {}: {e}", dir.display()),
        }
    }
    console::init(filter, plain, format);
}
