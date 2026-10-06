//! Served-byte accounting and background garbage collection.

use std::sync::Arc;

use tracing::{error, info};

use crate::storage::ServeStat;
use crate::types::{Counters, FileList};

use super::cluster::Cluster;

impl Cluster {
    /// Record bytes served by a download.
    pub async fn record_served(&self, stat: ServeStat) {
        let mut counters = self.counters.lock().await;
        counters.hits += stat.hits;
        counters.bytes += stat.bytes;
    }

    /// Snapshot the counters without resetting them.
    pub async fn take_counters_snapshot(&self) -> Counters {
        *self.counters.lock().await
    }

    /// Subtract the reported counters after a successful keep-alive.
    pub async fn subtract_counters(&self, reported: Counters) {
        let mut counters = self.counters.lock().await;
        counters.hits = counters.hits.saturating_sub(reported.hits);
        counters.bytes = counters.bytes.saturating_sub(reported.bytes);
    }

    /// Reclaim disk space in the background.
    pub fn gc_background(self: &Arc<Self>, files: FileList) {
        let cluster = Arc::clone(self);
        tokio::spawn(async move {
            match cluster.storage.gc(&files.files).await {
                Ok(counter) if counter.count == 0 => info!("no expired files"),
                Ok(counter) => info!(
                    count = counter.count,
                    bytes = counter.size,
                    "garbage collection complete"
                ),
                Err(e) => error!(error = %e, "gc error"),
            }
        });
    }
}
