//! Adaptive concurrency limiter (additive increase, multiplicative decrease).

use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Notify;

/// Discovers how much concurrency an upstream can actually take.
///
/// The cap grows by one on every success and is halved whenever the upstream
/// signals overload, mirroring TCP congestion control. This is what lets the
/// agent find the ceiling of a given AList/OpenList instance without anyone
/// having to guess a number in the configuration.
pub(crate) struct AdaptiveLimiter {
    limit: AtomicUsize,
    in_flight: AtomicUsize,
    min: usize,
    max: usize,
    notify: Notify,
}

impl AdaptiveLimiter {
    pub(crate) fn new(initial: usize, min: usize, max: usize) -> Self {
        let max = max.max(1);
        let min = min.clamp(1, max);
        AdaptiveLimiter {
            limit: AtomicUsize::new(initial.clamp(min, max)),
            in_flight: AtomicUsize::new(0),
            min,
            max,
            notify: Notify::new(),
        }
    }

    /// Wait for a free slot and take it. Dropping the permit releases it.
    pub(crate) async fn acquire(&self) -> Permit<'_> {
        loop {
            let in_flight = self.in_flight.load(Ordering::Acquire);
            if in_flight < self.limit.load(Ordering::Acquire)
                && self
                    .in_flight
                    .compare_exchange_weak(
                        in_flight,
                        in_flight + 1,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
            {
                return Permit { limiter: self };
            }
            // notify_one stores a permit when nobody is waiting, so a release
            // landing between the check above and this await is not lost.
            self.notify.notified().await;
        }
    }

    /// Record a success: allow one more concurrent request.
    pub(crate) fn on_success(&self) {
        let current = self.limit.load(Ordering::Acquire);
        if current < self.max {
            self.limit.store(current + 1, Ordering::Release);
            self.notify.notify_one();
        }
    }

    /// Record overload: halve the cap so the upstream can recover.
    pub(crate) fn on_overload(&self) {
        let current = self.limit.load(Ordering::Acquire);
        self.limit
            .store((current / 2).max(self.min), Ordering::Release);
    }

    /// Current cap.
    pub(crate) fn limit(&self) -> usize {
        self.limit.load(Ordering::Acquire)
    }
}

/// Releases an adaptive-limiter slot on drop.
pub(crate) struct Permit<'a> {
    limiter: &'a AdaptiveLimiter,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.limiter.in_flight.fetch_sub(1, Ordering::AcqRel);
        self.limiter.notify.notify_one();
    }
}

#[cfg(test)]
#[path = "limiter_test.rs"]
mod tests;
