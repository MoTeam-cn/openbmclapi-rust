//! Logging setup.
//!
//! The Node agent used pino with pino-pretty; the idiomatic Rust equivalent is
//! `tracing` with a `tracing-subscriber` formatter, which is what this is — not a
//! line-for-line port of pino. Two knobs stay compatible with the Node agent
//! (`LOGLEVEL` and `PLAIN_LOG`); `LOG_FORMAT` is new.

use tracing_subscriber::EnvFilter;

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
/// `level` mirrors the Node agent's `LOGLEVEL` (default `info`). `RUST_LOG` still
/// wins when it is set, so a targeted filter is always possible. `plain` (from
/// `PLAIN_LOG`) disables ANSI colouring; JSON output is never coloured.
pub fn init(level: &str, plain: bool, format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    // Ignore double initialisation (tests, embedded use).
    match format {
        LogFormat::Json => {
            let _ = builder.json().with_ansi(false).try_init();
        }
        LogFormat::Pretty => {
            let _ = builder.with_target(true).with_ansi(!plain).try_init();
        }
    }
}
