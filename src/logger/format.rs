//! The two log line shapes.
//!
//! Both mirror the Node agent. The application line is what pino-pretty
//! produced with `translateTime: 'SYS:standard'` and `singleLine: true`:
//!
//! ```
//! [2026-08-08 22:33:35.659 +0800] INFO (512242): booting openbmclapi 1.14.0
//! ```
//!
//! The access line is morgan's `combined` format, which is Apache's common log
//! format plus referrer and user agent:
//!
//! ```
//! 115.231.217.214 - - [08/Aug/2026:14:41:58 +0000] "GET /download/x HTTP/1.1" 302 - "-" "bmcurl/1.0"
//! ```

use std::fmt;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

/// Timestamp shape of the application line.
const APP_TIME: &str = "%Y-%m-%d %H:%M:%S%.3f %z";
/// Timestamp shape of the access line, as in the common log format.
const ACCESS_TIME: &str = "%d/%b/%Y:%H:%M:%S %z";

/// Which of the two shapes a layer produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// The agent's own line.
    App,
    /// The access line.
    Access,
}

/// `[time] LEVEL (pid): message`
pub struct AppFormat;

/// `host - - [time] "GET /x HTTP/1.1" 200 12 "-" "ua"`
pub struct AccessFormat;

impl<S, N> FormatEvent<S, N> for AppFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let level = event.metadata().level();
        let rendered = if writer.has_ansi_escapes() {
            format!("{}{}{}", level_colour(level), level, RESET)
        } else {
            level.to_string()
        };
        write!(
            writer,
            "{}",
            app_line(
                &now(APP_TIME),
                &rendered,
                std::process::id(),
                &Fields::from_event(event),
            )
        )
    }
}

impl<S, N> FormatEvent<S, N> for AccessFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        write!(
            writer,
            "{}",
            access_line(&now(ACCESS_TIME), &Fields::from_event(event))
        )
    }
}

const RESET: &str = "\u{1b}[0m";

/// Colour for a level, following pino-pretty's palette.
fn level_colour(level: &Level) -> &'static str {
    match *level {
        Level::ERROR => "\u{1b}[31m",
        Level::WARN => "\u{1b}[33m",
        Level::INFO => "\u{1b}[32m",
        Level::DEBUG => "\u{1b}[34m",
        Level::TRACE => "\u{1b}[35m",
    }
}

fn now(shape: &str) -> String {
    chrono::Local::now().format(shape).to_string()
}

/// Render the application line, newline included.
pub(crate) fn app_line(time: &str, level: &str, pid: u32, fields: &Fields) -> String {
    let mut line = format!("[{time}] {level} ({pid}): {}", fields.message);
    for (key, value) in &fields.named {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(value);
    }
    line.push('\n');
    line
}

/// Render the access line, newline included.
pub(crate) fn access_line(time: &str, fields: &Fields) -> String {
    let value = |name: &str| fields.get(name).unwrap_or("-");
    format!(
        "{} - - [{}] \"{} {} HTTP/{}\" {} {} \"{}\" \"{}\"\n",
        value("remote"),
        time,
        value("method"),
        value("uri"),
        value("version"),
        value("status"),
        value("length"),
        value("referer"),
        value("user_agent"),
    )
}

/// The fields of one event, split into the message and everything else.
#[derive(Debug, Default)]
pub(crate) struct Fields {
    /// The `message` field, rendered without a name.
    pub(crate) message: String,
    /// Every other field, in the order the event declared it.
    pub(crate) named: Vec<(String, String)>,
}

impl Fields {
    fn from_event(event: &Event<'_>) -> Self {
        let mut fields = Fields::default();
        event.record(&mut fields);
        fields
    }

    /// The value of a named field, if the event carried it.
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.named
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let rendered = format!("{value:?}");
        if field.name() == "message" {
            self.message = rendered;
        } else {
            self.named.push((field.name().to_string(), rendered));
        }
    }
}

#[cfg(test)]
#[path = "format_test.rs"]
mod tests;
