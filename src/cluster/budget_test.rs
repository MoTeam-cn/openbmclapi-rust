use std::time::Duration;

use super::{ByteBudget, GRANULARITY};

#[test]
fn rounds_the_budget_up_to_whole_units() {
    let budget = ByteBudget::new(GRANULARITY * 4 + 1);
    assert_eq!(budget.total_bytes(), GRANULARITY * 5);
}

#[test]
fn a_budget_never_rounds_down_to_nothing() {
    assert_eq!(ByteBudget::new(0).total_bytes(), GRANULARITY);
}

#[test]
fn a_small_download_takes_one_unit() {
    let budget = ByteBudget::new(GRANULARITY * 8);
    assert_eq!(budget.permits_for(0), 1);
    assert_eq!(budget.permits_for(1), 1);
    assert_eq!(budget.permits_for(GRANULARITY), 1);
    assert_eq!(budget.permits_for(GRANULARITY + 1), 2);
}

#[test]
fn an_oversized_download_takes_the_whole_budget() {
    let budget = ByteBudget::new(GRANULARITY * 4);
    assert_eq!(budget.permits_for(GRANULARITY * 100), 4);
}

#[tokio::test]
async fn an_oversized_download_still_completes() {
    let budget = ByteBudget::new(GRANULARITY * 4);
    let _guard = budget.acquire(GRANULARITY * 100).await;
}

#[tokio::test]
async fn a_second_large_download_waits_for_the_first() {
    let budget = ByteBudget::new(GRANULARITY * 4);
    let first = budget.acquire(GRANULARITY * 4).await;
    let blocked =
        tokio::time::timeout(Duration::from_millis(50), budget.acquire(GRANULARITY * 4)).await;
    assert!(
        blocked.is_err(),
        "a fully taken budget must not hand out more"
    );
    drop(first);
    assert!(
        tokio::time::timeout(Duration::from_millis(500), budget.acquire(GRANULARITY * 4))
            .await
            .is_ok(),
        "dropping the guard must release the budget"
    );
}
