//! Per-category log files.
//!
//! Events are routed by target and level: request lines to `access.log`, sync
//! activity to `sync.log`, anything at WARN or above to `error.log`, and the
//! remainder to `agent.log`. Files are opened in append mode, so a restart
//! continues the history rather than truncating it; rotation is the operator's
//! job, which is what makes this work with logrotate and friends.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use tracing::Level;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use super::LogFormat;

/// Target the request-logging middleware emits under.
pub const ACCESS_TARGET: &str = "openbmclapi::access";

/// Module path that owns file synchronisation.
const SYNC_TARGET: &str = "openbmclapi::cluster::sync";

/// Whether an event belongs in `access.log`.
pub fn is_access(target: &str) -> bool {
    target == ACCESS_TARGET
}

/// Whether an event belongs in `sync.log`.
pub fn is_sync(target: &str) -> bool {
    target.starts_with(SYNC_TARGET)
}

/// Whether an event belongs in `error.log`.
///
/// Matched explicitly rather than compared: `tracing::Level` orders itself the
/// other way round, so `ERROR` sorts *below* `WARN` and a `>=` test would
/// quietly drop the errors.
pub fn is_error(level: &Level) -> bool {
    matches!(level, &Level::WARN | &Level::ERROR)
}

/// Whether an event belongs in `agent.log`, which keeps everything else.
pub fn is_agent(target: &str) -> bool {
    !is_access(target) && !is_sync(target)
}

/// Append-only handle shared by every writer of one file.
#[derive(Clone)]
struct SharedFile {
    file: Arc<Mutex<File>>,
}

impl SharedFile {
    fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(SharedFile {
            file: Arc::new(Mutex::new(file)),
        })
    }
}

/// One locked file handle, as `MakeWriter` requires.
struct FileGuard<'a> {
    guard: MutexGuard<'a, File>,
}

impl Write for FileGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.guard.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.guard.flush()
    }
}

impl<'a> MakeWriter<'a> for SharedFile {
    type Writer = FileGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        FileGuard {
            guard: self
                .file
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        }
    }
}

/// Open the four files and install the subscriber.
/// The four category files, opened for append.
struct Files {
    access: SharedFile,
    sync: SharedFile,
    error: SharedFile,
    agent: SharedFile,
}

/// Create the directory and open one file per category.
fn open(dir: &Path) -> io::Result<Files> {
    std::fs::create_dir_all(dir)?;
    Ok(Files {
        access: SharedFile::open(&dir.join("access.log"))?,
        sync: SharedFile::open(&dir.join("sync.log"))?,
        error: SharedFile::open(&dir.join("error.log"))?,
        agent: SharedFile::open(&dir.join("agent.log"))?,
    })
}

pub(super) fn init(
    filter: EnvFilter,
    plain: bool,
    format: LogFormat,
    dir: &Path,
) -> io::Result<()> {
    let Files {
        access,
        sync,
        error,
        agent,
    } = open(dir)?;

    let json = format == LogFormat::Json;
    // Ignore double initialisation (tests, embedded use).
    let _ =
        tracing_subscriber::registry()
            .with(filter)
            .with(console_layer(json, plain))
            .with(file_layer(access, json).with_filter(filter_fn(
                |meta: &tracing::Metadata<'_>| is_access(meta.target()),
            )))
            .with(
                file_layer(sync, json).with_filter(filter_fn(|meta: &tracing::Metadata<'_>| {
                    is_sync(meta.target())
                })),
            )
            .with(
                file_layer(error, json).with_filter(filter_fn(|meta: &tracing::Metadata<'_>| {
                    is_error(meta.level())
                })),
            )
            .with(
                file_layer(agent, json).with_filter(filter_fn(|meta: &tracing::Metadata<'_>| {
                    is_agent(meta.target())
                })),
            )
            .try_init();
    Ok(())
}

/// The console layer, in whichever shape `LOG_FORMAT` asked for.
fn console_layer<S>(json: bool, plain: bool) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    if json {
        Box::new(fmt::layer().json().with_ansi(false))
    } else {
        Box::new(fmt::layer().with_target(true).with_ansi(!plain))
    }
}

/// One file layer, writing to a single category file.
fn file_layer<S>(file: SharedFile, json: bool) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    if json {
        Box::new(fmt::layer().with_writer(file).with_ansi(false).json())
    } else {
        Box::new(
            fmt::layer()
                .with_writer(file)
                .with_ansi(false)
                .with_target(true),
        )
    }
}

#[cfg(test)]
#[path = "files_test.rs"]
mod tests;
