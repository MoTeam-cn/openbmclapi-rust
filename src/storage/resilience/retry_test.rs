use std::time::Duration;

use axum::http::{HeaderMap, HeaderValue};

use super::{classify_status, retry_after, Disposition, RetryPolicy};

#[test]
fn classifies_overload_as_retryable() {
    for status in [408, 425, 500, 502, 503, 504, 507, 509] {
        assert_eq!(
            classify_status(status),
            Disposition::Retryable,
            "status {status} should be retryable"
        );
    }
    assert_eq!(classify_status(404), Disposition::NotFound);
    assert_eq!(classify_status(410), Disposition::NotFound);
    assert_eq!(classify_status(403), Disposition::Permanent);
    assert_eq!(classify_status(200), Disposition::Permanent);
}

#[test]
fn treats_429_as_a_lockout_rather_than_load() {
    assert_eq!(
        classify_status(429),
        Disposition::Throttled,
        "an auth lockout must not be retried"
    );
}

#[test]
fn reads_retry_after_in_seconds() {
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("7"));
    assert_eq!(retry_after(&headers), Some(Duration::from_secs(7)));
}

#[test]
fn ignores_a_missing_or_bogus_retry_after() {
    assert_eq!(retry_after(&HeaderMap::new()), None);
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("soon"));
    assert_eq!(retry_after(&headers), None);
}

#[test]
fn backoff_grows_and_is_capped() {
    let policy = RetryPolicy::new(5, Duration::from_millis(100), Duration::from_secs(2));
    let first = policy.delay(1, None);
    assert!(first >= Duration::from_millis(50), "got {first:?}");
    assert!(first <= Duration::from_millis(150), "got {first:?}");
    assert!(policy.delay(20, None) <= Duration::from_secs(2));
}

#[test]
fn retry_after_wins_over_the_backoff() {
    let policy = RetryPolicy::new(5, Duration::from_millis(100), Duration::from_secs(2));
    assert_eq!(
        policy.delay(1, Some(Duration::from_secs(9))),
        Duration::from_secs(2)
    );
    assert_eq!(
        policy.delay(1, Some(Duration::from_millis(300))),
        Duration::from_millis(300)
    );
}
