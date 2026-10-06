//! Failure classification and bounded retry with backoff.

use std::time::Duration;

use axum::http::{header, HeaderMap};

/// What the agent should do about an upstream failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// Transient: the same request may succeed later.
    Retryable,
    /// The object is absent; retrying is pointless.
    NotFound,
    /// The upstream answered definitively; do not retry.
    Permanent,
    /// The upstream refused on purpose (auth lockout / throttling).
    ///
    /// Retrying immediately makes it worse, so the client has to back off as a
    /// whole instead of repeating this request.
    Throttled,
}

/// Classify an upstream HTTP status.
///
/// 500/502/503/504 count as retryable because for a self-hosted object store
/// they almost always mean "overloaded" rather than "broken request".
pub(crate) fn classify_status(status: u16) -> Disposition {
    match status {
        404 | 410 => Disposition::NotFound,
        // AList/OpenList answer 429 when the WebDAV auth lockout trips, not when
        // they are merely busy: repeating the request keeps the lockout alive.
        429 => Disposition::Throttled,
        408 | 425 | 500 | 502 | 503 | 504 | 507 | 509 => Disposition::Retryable,
        _ => Disposition::Permanent,
    }
}

/// Classify a transport-level failure.
pub(crate) fn classify_error(error: &reqwest::Error) -> Disposition {
    if error.is_timeout()
        || error.is_connect()
        || error.is_request()
        || error.is_body()
        || error.is_decode()
    {
        Disposition::Retryable
    } else {
        Disposition::Permanent
    }
}

/// Parse a `Retry-After` header, in either delta-seconds or HTTP-date form.
pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let raw = headers.get(header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let when = chrono::DateTime::parse_from_rfc2822(raw).ok()?;
    (when.with_timezone(&chrono::Utc) - chrono::Utc::now())
        .to_std()
        .ok()
        .filter(|delay| !delay.is_zero())
}

/// Bounded exponential backoff with full jitter.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy {
    max_attempts: u32,
    base: Duration,
    cap: Duration,
}

impl RetryPolicy {
    pub(crate) fn new(max_attempts: u32, base: Duration, cap: Duration) -> Self {
        RetryPolicy {
            max_attempts: max_attempts.max(1),
            base,
            cap,
        }
    }

    pub(crate) fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// How long to wait before the retry that follows the given number of failures.
    ///
    /// An explicit `Retry-After` always wins; otherwise the delay doubles and
    /// is jittered so a fleet of agents does not retry in lockstep.
    pub(crate) fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(hint) = retry_after {
            return hint.min(self.cap);
        }
        let factor = 2u32.saturating_pow(attempt.saturating_sub(1).min(10));
        let raw = self.base.saturating_mul(factor).min(self.cap);
        raw.mul_f64(0.5 + rand::random::<f64>()).min(self.cap)
    }
}

#[cfg(test)]
#[path = "retry_test.rs"]
mod tests;
