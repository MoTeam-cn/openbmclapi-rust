//! Console-only output.

use tracing_subscriber::EnvFilter;

use super::LogFormat;

/// Install a subscriber that writes to stdout and stderr only.
pub(super) fn init(filter: EnvFilter, plain: bool, format: LogFormat) {
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
