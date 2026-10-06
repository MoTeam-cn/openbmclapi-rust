//! Periodic `keep-alive` reporting and recovery.
//!
//! Mirrors `src/keepalive.ts`: every minute the agent reports the bytes it has
//! served, and three consecutive failures (or a "kicked" response) trigger a
//! full disable/connect/enable cycle.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use serde_json::json;
use std::sync::Mutex;

use tokio::task::JoinHandle;
use tracing::{error, info, trace};

use crate::cluster::Cluster;
use crate::error::{Error, Result};

const REPORT_INTERVAL: Duration = Duration::from_secs(60);
const REPORT_TIMEOUT: Duration = Duration::from_secs(10);
const RESTART_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_ERRORS: u32 = 3;

/// Drives the keep-alive loop for an enabled cluster.
pub struct Keepalive {
    interval: Duration,
    cluster: Weak<Cluster>,
    task: Mutex<Option<JoinHandle<()>>>,
    errors: AtomicU32,
}

impl Keepalive {
    pub fn new(cluster: Weak<Cluster>) -> Self {
        Keepalive {
            interval: REPORT_INTERVAL,
            cluster,
            task: Mutex::new(None),
            errors: AtomicU32::new(0),
        }
    }

    /// Start reporting on the given interval.
    ///
    /// Deliberately synchronous: `Cluster::enable` calls this, and making it
    /// async would create a cycle in the compiler's `Send` inference (the
    /// spawned task reaches back into `enable` through `restart`).
    pub fn start(self: &Arc<Self>) {
        self.stop();
        let this = Arc::clone(self);
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(this.interval).await;
                trace!("发送心跳");
                let outcome = tokio::time::timeout(REPORT_TIMEOUT, this.emit_keep_alive()).await;
                match outcome {
                    Ok(Ok(true)) => {
                        this.errors.store(0, Ordering::Relaxed);
                    }
                    Ok(Ok(false)) => {
                        info!("被主控踢下线，正在重启");
                        this.restart().await;
                    }
                    Ok(Err(e)) => {
                        let count = this.errors.fetch_add(1, Ordering::Relaxed) + 1;
                        error!(error = %e, count, "心跳出错");
                        if count >= MAX_ERRORS {
                            this.restart().await;
                        }
                    }
                    Err(_) => {
                        let count = this.errors.fetch_add(1, Ordering::Relaxed) + 1;
                        error!(count, "心跳超时");
                        if count >= MAX_ERRORS {
                            this.restart().await;
                        }
                    }
                }
            }
        });
        *self.task.lock().expect("keepalive task lock poisoned") = Some(handle);
    }

    /// Stop reporting.
    pub fn stop(&self) {
        if let Some(handle) = self
            .task
            .lock()
            .expect("keepalive task lock poisoned")
            .take()
        {
            handle.abort();
        }
    }

    async fn emit_keep_alive(&self) -> Result<bool> {
        let Some(cluster) = self.cluster.upgrade() else {
            return Err(Error::Other("cluster dropped".into()));
        };
        if !cluster.is_enabled() {
            return Err(Error::Other("cluster is not enabled".into()));
        }
        let counters = cluster.take_counters_snapshot().await;
        let payload = json!({
            "time": chrono::Utc::now().to_rfc3339(),
            "hits": counters.hits,
            "bytes": counters.bytes,
        });
        let ack = cluster
            .socket
            .emit_with_ack("keep-alive", payload, REPORT_TIMEOUT)
            .await?;
        let (err, date) = split_ack(&ack);
        if let Some(err) = err {
            return Err(Error::Service(err.to_string()));
        }
        info!(hits = counters.hits, bytes = counters.bytes, "心跳成功");
        cluster.subtract_counters(counters).await;
        Ok(date.is_some_and(|v| !v.is_null() && v != &serde_json::Value::Bool(false)))
    }

    async fn restart(&self) {
        let Some(cluster) = self.cluster.upgrade() else {
            return;
        };
        let attempt = async {
            if let Err(e) = cluster.disable().await {
                error!(error = %e, "重启前注销失败");
            }
            cluster.connect().await;
            cluster.enable().await
        };
        match tokio::time::timeout(RESTART_TIMEOUT, attempt).await {
            Ok(Ok(())) => info!("集群已重启"),
            Ok(Err(e)) => {
                error!(error = %e, "重启失败");
                cluster.exit(1);
            }
            Err(_) => {
                error!("重启超时");
                cluster.exit(1);
            }
        }
    }
}

/// Split the socket.io callback argument array into `(error, value)`.
pub fn split_ack(
    ack: &serde_json::Value,
) -> (Option<&serde_json::Value>, Option<&serde_json::Value>) {
    match ack {
        serde_json::Value::Array(items) => {
            let err = items.first().filter(|v| !v.is_null());
            let value = items.get(1);
            (err, value)
        }
        other => (None, Some(other)),
    }
}

#[cfg(test)]
#[path = "keepalive_test.rs"]
mod tests;
