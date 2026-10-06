//! A `reqwest` client wrapper that applies the resilience policy.

use std::sync::Arc;
use std::time::Duration;

use crate::error::{Error, Result};

use super::breaker::{Admission, CircuitBreaker};
use super::limiter::AdaptiveLimiter;
use super::retry::{classify_error, classify_status, retry_after, Disposition, RetryPolicy};

/// Tuning for the resilient client wrapper.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResilienceConfig {
    /// Consecutive failures before the breaker opens.
    pub(crate) breaker_threshold: u32,
    /// How long the breaker refuses calls before probing again.
    pub(crate) breaker_cooldown: Duration,
    /// Attempts per logical request, including the first.
    pub(crate) max_attempts: u32,
    /// First backoff delay; it doubles on every further attempt.
    pub(crate) retry_base: Duration,
    /// Upper bound on any single backoff, including a `Retry-After` hint.
    pub(crate) retry_cap: Duration,
    /// Concurrency the adaptive limiter starts at.
    pub(crate) concurrency_initial: usize,
    /// Floor the limiter will never go below.
    pub(crate) concurrency_min: usize,
    /// Ceiling the limiter will never exceed.
    pub(crate) concurrency_max: usize,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        ResilienceConfig {
            breaker_threshold: 5,
            breaker_cooldown: Duration::from_secs(15),
            max_attempts: 3,
            retry_base: Duration::from_millis(250),
            retry_cap: Duration::from_secs(30),
            concurrency_initial: 8,
            concurrency_min: 1,
            // AList/OpenList serves WebDAV from one HTTP/1.1 listener and the
            // Node agent fanned out 10 at a time; stay in that ballpark.
            concurrency_max: 16,
        }
    }
}

/// A `reqwest::Client` plus a circuit breaker, an adaptive concurrency cap
/// and bounded retries.
#[derive(Clone)]
pub(crate) struct ResilientClient {
    http: reqwest::Client,
    breaker: Arc<CircuitBreaker>,
    limiter: Arc<AdaptiveLimiter>,
    retry: RetryPolicy,
}

impl ResilientClient {
    pub(crate) fn new(http: reqwest::Client, config: ResilienceConfig) -> Self {
        ResilientClient {
            http,
            breaker: Arc::new(CircuitBreaker::new(
                config.breaker_threshold,
                config.breaker_cooldown,
            )),
            limiter: Arc::new(AdaptiveLimiter::new(
                config.concurrency_initial,
                config.concurrency_min,
                config.concurrency_max,
            )),
            retry: RetryPolicy::new(config.max_attempts, config.retry_base, config.retry_cap),
        }
    }

    /// The underlying client, for requests that must not be retried here.
    pub(crate) fn raw(&self) -> &reqwest::Client {
        &self.http
    }

    /// Current adaptive concurrency cap, for logging and tests.
    pub(crate) fn concurrency_limit(&self) -> usize {
        self.limiter.limit()
    }

    /// Send one logical request through the policy.
    ///
    /// `build` runs once per attempt, so it must produce a fresh builder.
    /// A retryable status or transport error is retried up to the configured
    /// budget; an open breaker fails fast with
    /// `Error::UpstreamUnavailable` so the caller can answer 503 instead of
    /// queueing more work onto a struggling upstream.
    pub(crate) async fn send<F>(&self, url: &str, build: F) -> Result<reqwest::Response>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        let mut attempt = 0;
        loop {
            attempt += 1;
            if self.breaker.admit() == Admission::Open {
                let retry_in_ms = self.breaker.retry_in().unwrap_or_default().as_millis() as u64;
                return Err(Error::UpstreamUnavailable { retry_in_ms });
            }

            let permit = self.limiter.acquire().await;
            let outcome = build().send().await;
            drop(permit);

            match outcome {
                Ok(response) => {
                    let status = response.status().as_u16();
                    match classify_status(status) {
                        Disposition::Retryable => {
                            self.breaker.record_failure();
                            self.limiter.on_overload();
                            if attempt >= self.retry.max_attempts() {
                                return Err(Error::Status {
                                    status,
                                    url: url.to_string(),
                                });
                            }
                            let hint = retry_after(response.headers());
                            // Release the connection before sleeping on it.
                            drop(response);
                            tokio::time::sleep(self.retry.delay(attempt, hint)).await;
                        }
                        Disposition::Throttled => {
                            // Repeating this request would only prolong the
                            // upstream's own lockout, so fail fast and keep
                            // everyone else away while it cools off.
                            self.breaker.trip();
                            self.limiter.on_overload();
                            let retry_in_ms = retry_after(response.headers())
                                .or_else(|| self.breaker.retry_in())
                                .unwrap_or_default()
                                .as_millis() as u64;
                            return Err(Error::UpstreamUnavailable { retry_in_ms });
                        }
                        _ => {
                            self.breaker.record_success();
                            self.limiter.on_success();
                            return Ok(response);
                        }
                    }
                }
                Err(error) => {
                    let retryable = classify_error(&error) == Disposition::Retryable;
                    self.breaker.record_failure();
                    if !retryable || attempt >= self.retry.max_attempts() {
                        return Err(error.into());
                    }
                    self.limiter.on_overload();
                    tokio::time::sleep(self.retry.delay(attempt, None)).await;
                }
            }
        }
    }
}
