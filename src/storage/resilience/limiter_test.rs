use std::time::Duration;

use super::AdaptiveLimiter;

#[tokio::test]
async fn a_single_slot_is_released_when_the_permit_drops() {
    let limiter = AdaptiveLimiter::new(1, 1, 1);
    let permit = limiter.acquire().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), limiter.acquire())
            .await
            .is_err(),
        "the second acquire must wait for the only slot"
    );
    drop(permit);
    let _again = tokio::time::timeout(Duration::from_millis(50), limiter.acquire())
        .await
        .expect("the slot must be free again");
}

#[tokio::test]
async fn grows_on_success_and_halves_on_overload() {
    let limiter = AdaptiveLimiter::new(4, 1, 8);
    limiter.on_success();
    assert_eq!(limiter.limit(), 5);
    limiter.on_overload();
    assert_eq!(limiter.limit(), 2);
    limiter.on_overload();
    assert_eq!(limiter.limit(), 1);
    limiter.on_overload();
    assert_eq!(limiter.limit(), 1, "never drops below the minimum");
}

#[tokio::test]
async fn never_exceeds_the_maximum() {
    let limiter = AdaptiveLimiter::new(2, 1, 2);
    limiter.on_success();
    limiter.on_success();
    assert_eq!(limiter.limit(), 2);
}
