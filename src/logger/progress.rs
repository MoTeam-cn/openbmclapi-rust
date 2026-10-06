//! The two-bar sync display the Node agent drew with cli-progress.
//!
//! Two lines are redrawn in place: one over the file count, one over the bytes
//! of the object being fetched.
//!
//! ```
//!  =============--------------------------- | files | 1/3
//!  ======================================== | /maven/net/neoforged/x.jar | 764421/764421
//! ```
//!
//! Downloads run several at a time, so the byte bar shows the oldest object
//! still in flight and every object keeps its own counter; a single shared
//! counter would mix unrelated files together. A live redraw only makes sense
//! on a terminal, so a piped run — or `PLAIN_LOG` — keeps the periodic
//! progress log line instead.

use std::io::{IsTerminal, Write};
use std::sync::{Mutex, MutexGuard};

/// Width of the bar itself, matching cli-progress's default.
const BAR: usize = 40;

/// Two bars: files done overall, and bytes of the file on show.
pub struct Progress {
    inner: Option<Mutex<State>>,
}

/// Handle for one in-flight object, so its bytes land on its own counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileBar(u64);

#[derive(Default)]
struct State {
    files_done: u64,
    files_total: u64,
    next_id: u64,
    active: Vec<Active>,
    /// Lines currently on screen, so the next redraw can climb back over them.
    drawn: usize,
    /// Suppresses drawing, so the state tests do not scribble on stderr.
    silent: bool,
}

/// One object being downloaded.
struct Active {
    id: u64,
    label: String,
    done: u64,
    total: u64,
}

impl Progress {
    /// Build the display. `enabled` comes from the configuration; drawing also
    /// needs a terminal, so anything piped silently falls back to logging.
    pub fn new(files_total: u64, enabled: bool) -> Self {
        let live = enabled && std::io::stderr().is_terminal();
        Progress {
            inner: live.then(|| {
                Mutex::new(State {
                    files_total,
                    ..State::default()
                })
            }),
        }
    }

    /// Whether the bars are being drawn, as opposed to the fallback log line.
    pub fn is_live(&self) -> bool {
        self.inner.is_some()
    }

    /// Register an object as in flight and return its handle.
    pub fn start_file(&self, label: &str, bytes_total: u64) -> FileBar {
        let Some(inner) = &self.inner else {
            return FileBar(0);
        };
        let mut state = lock(inner);
        state.next_id += 1;
        let id = state.next_id;
        state.active.push(Active {
            id,
            label: label.to_string(),
            done: 0,
            total: bytes_total,
        });
        draw(&mut state);
        FileBar(id)
    }

    /// Record more bytes of the object `bar` belongs to.
    pub fn add_bytes(&self, bar: FileBar, bytes: u64) {
        let Some(inner) = &self.inner else {
            return;
        };
        let mut state = lock(inner);
        if let Some(active) = state.active.iter_mut().find(|active| active.id == bar.0) {
            active.done = active.done.saturating_add(bytes);
        }
        draw(&mut state);
    }

    /// Retire an object and count it towards the overall bar.
    pub fn finish_file(&self, bar: FileBar) {
        let Some(inner) = &self.inner else {
            return;
        };
        let mut state = lock(inner);
        state.active.retain(|active| active.id != bar.0);
        state.files_done = state.files_done.saturating_add(1).min(state.files_total);
        draw(&mut state);
    }

    /// Wipe the bars, leaving the cursor where the next log line goes.
    pub fn finish(&self) {
        let Some(inner) = &self.inner else {
            return;
        };
        let mut state = lock(inner);
        if state.drawn == 0 {
            return;
        }
        let mut out = std::io::stderr().lock();
        let _ = write!(out, "\r\u{1b}[2K");
        for _ in 1..state.drawn {
            let _ = write!(out, "\u{1b}[1A\r\u{1b}[2K");
        }
        let _ = out.flush();
        state.drawn = 0;
    }

    /// A display that does not need a terminal, for the state tests.
    #[cfg(test)]
    pub(crate) fn forced(files_total: u64) -> Self {
        Progress {
            inner: Some(Mutex::new(State {
                files_total,
                silent: true,
                ..State::default()
            })),
        }
    }

    /// How many files are done and how many are still in flight.
    #[cfg(test)]
    pub(crate) fn counters(&self) -> (u64, usize) {
        let Some(inner) = &self.inner else {
            return (0, 0);
        };
        let state = lock(inner);
        (state.files_done, state.active.len())
    }

    /// Bytes recorded against one handle.
    #[cfg(test)]
    pub(crate) fn done_bytes(&self, bar: FileBar) -> u64 {
        let Some(inner) = &self.inner else {
            return 0;
        };
        let state = lock(inner);
        state
            .active
            .iter()
            .find(|active| active.id == bar.0)
            .map(|active| active.done)
            .unwrap_or(0)
    }
}

fn lock(inner: &Mutex<State>) -> MutexGuard<'_, State> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Redraw both bars in place.
fn draw(state: &mut State) {
    if state.silent {
        return;
    }
    let mut out = std::io::stderr().lock();
    let up = state.drawn.saturating_sub(1);
    if up > 0 {
        let _ = write!(out, "\u{1b}[{up}A");
    }
    let overall = overall_line(state.files_done, state.files_total);
    let file = match state.active.first() {
        Some(active) => file_line(&active.label, active.done, active.total),
        None => file_line("-", 0, 0),
    };
    let _ = write!(out, "\r\u{1b}[2K{overall}\n\r\u{1b}[2K{file}");
    let _ = out.flush();
    state.drawn = 2;
}

/// The overall bar line.
pub(crate) fn overall_line(done: u64, total: u64) -> String {
    format!("{} | files | {done}/{total}", bar(ratio(done, total)))
}

/// The per-file bar line.
pub(crate) fn file_line(label: &str, done: u64, total: u64) -> String {
    format!("{} | {label} | {done}/{total}", bar(ratio(done, total)))
}

/// A fraction in `0.0..=1.0`; nothing to do reads as complete.
fn ratio(done: u64, total: u64) -> f64 {
    if total == 0 {
        return 1.0;
    }
    (done as f64 / total as f64).clamp(0.0, 1.0)
}

/// ` =============---------------------------`
fn bar(fraction: f64) -> String {
    let filled = (fraction * BAR as f64).round() as usize;
    let mut out = String::with_capacity(BAR + 1);
    out.push(' ');
    for index in 0..BAR {
        out.push(if index < filled { '=' } else { '-' });
    }
    out
}

#[cfg(test)]
#[path = "progress_test.rs"]
mod tests;
