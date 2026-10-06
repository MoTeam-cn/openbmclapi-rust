//! Console output.

use tracing_subscriber::fmt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;

use super::files::is_access;
use super::format::{AccessFormat, AppFormat, Line};
use super::LogFormat;

/// One console layer, producing `line`.
///
/// The access line and the application line are different shapes, so the
/// console is two layers with complementary filters rather than one.
pub(super) fn layer<S>(
    plain: bool,
    format: LogFormat,
    line: Line,
) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    match (format, line) {
        (LogFormat::Json, _) => Box::new(fmt::layer().json().with_ansi(false)),
        (LogFormat::Pretty, Line::Access) => {
            Box::new(fmt::layer().event_format(AccessFormat).with_ansi(!plain))
        }
        (LogFormat::Pretty, Line::App) => {
            Box::new(fmt::layer().event_format(AppFormat).with_ansi(!plain))
        }
    }
}

/// Install a subscriber that writes to the console only.
pub(super) fn init(filter: EnvFilter, plain: bool, format: LogFormat) {
    use tracing_subscriber::filter::filter_fn;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    // Ignore double initialisation (tests, embedded use).
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer(plain, format, Line::Access).with_filter(filter_fn(access_only)))
        .with(layer(plain, format, Line::App).with_filter(filter_fn(app_only)))
        .try_init();
}

fn access_only(meta: &tracing::Metadata<'_>) -> bool {
    is_access(meta.target())
}

fn app_only(meta: &tracing::Metadata<'_>) -> bool {
    !is_access(meta.target())
}
