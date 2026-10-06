use std::time::Duration;

use super::{Admission, CircuitBreaker};

#[test]
fn stays_closed_below_the_threshold() {
    let breaker = CircuitBreaker::new(3, Duration::from_secs(1));
    breaker.record_failure();
    breaker.record_failure();
    assert_eq!(breaker.admit(), Admission::Allowed);
    assert!(breaker.retry_in().is_none());
}

#[test]
fn opens_at_the_threshold_and_admits_one_probe_after_the_cooldown() {
    let breaker = CircuitBreaker::new(2, Duration::from_millis(1));
    breaker.record_failure();
    breaker.record_failure();
    assert_eq!(breaker.admit(), Admission::Open);

    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(
        breaker.admit(),
        Admission::Allowed,
        "one probe is let through"
    );
    assert_eq!(
        breaker.admit(),
        Admission::Open,
        "the second caller still fails fast"
    );

    breaker.record_success();
    assert_eq!(breaker.admit(), Admission::Allowed);
}

#[test]
fn a_failed_probe_restarts_the_cooldown() {
    let breaker = CircuitBreaker::new(2, Duration::from_millis(1));
    breaker.record_failure();
    breaker.record_failure();
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(breaker.admit(), Admission::Allowed);
    breaker.record_failure();
    assert_eq!(breaker.admit(), Admission::Open);
    assert!(breaker.retry_in().is_some());
}
#[test]
fn trip_opens_the_breaker_immediately() {
    let breaker = CircuitBreaker::new(10, Duration::from_secs(30));
    assert_eq!(breaker.admit(), Admission::Allowed);
    breaker.trip();
    assert_eq!(breaker.admit(), Admission::Open);
    assert!(breaker.retry_in().is_some());
}
