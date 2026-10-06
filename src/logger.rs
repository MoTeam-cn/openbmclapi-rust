use tracing_subscriber::EnvFilter;

/// Initialise the global tracing subscriber.
///
/// `level` corresponds to the `LOGLEVEL` environment variable of the Node
/// implementation (default `info`). `plain` disables ANSI colouring and is
/// driven by `PLAIN_LOG`.
pub fn init(level: &str, plain: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_ansi(!plain);
    // Ignore double initialisation (tests, embedded use).
    let _ = builder.try_init();
}
