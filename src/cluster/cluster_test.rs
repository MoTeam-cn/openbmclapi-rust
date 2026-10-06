use super::*;

/// The cluster is shared across tasks, so this must keep holding.
#[test]
fn cluster_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Cluster>();
}
