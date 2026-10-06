//! Consecutive-failure circuit breaker.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Whether a call may proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// The upstream looks healthy, or this caller is the recovery probe.
    Allowed,
    /// The breaker is open; the caller should fail fast instead of waiting.
    Open,
}

#[derive(Debug, Default)]
struct State {
    consecutive_failures: u32,
    opened_at: Option<Instant>,
    probing: bool,
}

/// Stops hammering an upstream that is already failing.
///
/// While open, every call is refused immediately for the configured cooldown;
/// after that a single probe is let through, and its outcome either closes the
/// breaker again or restarts the cooldown.
pub(crate) struct CircuitBreaker {
    state: Mutex<State>,
    threshold: u32,
    cooldown: Duration,
}

impl CircuitBreaker {
    pub(crate) fn new(threshold: u32, cooldown: Duration) -> Self {
        CircuitBreaker {
            state: Mutex::new(State::default()),
            threshold: threshold.max(1),
            cooldown,
        }
    }

    /// Ask to proceed.
    pub(crate) fn admit(&self) -> Admission {
        let mut state = self.lock();
        match state.opened_at {
            None => Admission::Allowed,
            Some(opened) => {
                if opened.elapsed() >= self.cooldown && !state.probing {
                    state.probing = true;
                    Admission::Allowed
                } else {
                    Admission::Open
                }
            }
        }
    }

    pub(crate) fn record_success(&self) {
        *self.lock() = State::default();
    }

    /// Open the breaker immediately, regardless of the failure count.
    ///
    /// Used when the upstream explicitly says "go away" (an auth lockout, a
    /// throttling response): waiting for the threshold would keep hammering it.
    pub(crate) fn trip(&self) {
        let mut state = self.lock();
        state.consecutive_failures = self.threshold;
        state.opened_at = Some(Instant::now());
        state.probing = false;
    }

    pub(crate) fn record_failure(&self) {
        let mut state = self.lock();
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        // A failed probe re-opens immediately; otherwise wait for the threshold.
        if state.consecutive_failures >= self.threshold || state.probing {
            state.opened_at = Some(Instant::now());
            state.probing = false;
        }
    }

    /// Time left before the next probe, for a `Retry-After` hint.
    /// Returns `None` while the breaker is closed.
    pub(crate) fn retry_in(&self) -> Option<Duration> {
        let state = self.lock();
        let opened = state.opened_at?;
        Some(self.cooldown.saturating_sub(opened.elapsed()))
    }

    /// A poisoned lock only means another thread panicked mid-update; the
    /// counters are still consistent, so carry on rather than panic too.
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
#[path = "breaker_test.rs"]
mod tests;
