//! TTL set backing the WebDAV existence cache.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Simple in-memory TTL set used for the existence cache.
#[derive(Default)]
pub(super) struct TtlCache {
    entries: Mutex<HashMap<String, Instant>>,
    ttl: Duration,
}

impl TtlCache {
    pub(super) fn new(ttl: Duration) -> Self {
        TtlCache {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    pub(super) async fn contains(&self, key: &str) -> bool {
        let guard = self.entries.lock().await;
        guard.get(key).is_some_and(|at| at.elapsed() < self.ttl)
    }

    pub(super) async fn insert(&self, key: &str) {
        self.entries
            .lock()
            .await
            .insert(key.to_string(), Instant::now());
    }

    pub(super) async fn remove(&self, key: &str) {
        self.entries.lock().await.remove(key);
    }
}
