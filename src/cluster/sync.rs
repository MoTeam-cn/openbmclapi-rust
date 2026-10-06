//! File synchronisation and on-demand object downloads.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tracing::{debug, error, info, trace, warn};

use crate::error::{Error, Result};
use crate::logger::progress::{FileBar, Progress};
use crate::types::{FileInfo, FileList, SyncConfig};

use super::budget::ByteBudget;
use super::cluster::Cluster;

/// Retries per file when syncing, matching `p-retry` in the Node agent.
const SYNC_RETRIES: u32 = 10;

/// Warn once free space falls below this fraction of the filesystem.
const LOW_SPACE_RATIO: f64 = 0.05;

impl Cluster {
    /// Download every file that is missing or size-mismatched.
    pub async fn sync_files(&self, file_list: &FileList, sync: &SyncConfig) -> Result<()> {
        if !self.storage.check().await? {
            return Err(Error::storage("存储不可写"));
        }
        info!("正在检查缺失文件");
        let missing = self.storage.get_missing_files(&file_list.files).await?;
        if missing.is_empty() {
            return Ok(());
        }
        info!(count = missing.len(), "发现不一致，开始同步");
        info!(concurrency = sync.concurrency, "同步策略");

        self.check_free_space(&missing).await?;

        let concurrency = sync.concurrency.max(1);
        let total = missing.len();
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let has_error = Arc::new(AtomicBool::new(false));
        let budget = ByteBudget::new(self.config.sync_memory_budget.saturating_mul(1024 * 1024));
        info!(budget_mib = self.config.sync_memory_budget, "下载内存预算");

        let progress = Progress::new(total as u64, !self.config.plain_log);
        let results = futures::stream::iter(missing)
            .map(|file| {
                let done = Arc::clone(&done);
                let has_error = Arc::clone(&has_error);
                let budget = &budget;
                let progress = &progress;
                async move {
                    // Hold the reservation for the whole download: the body is
                    // buffered so its checksum can be verified, so the budget
                    // has to cover it until the write finishes.
                    let _reservation = budget.acquire(file.size.max(0) as u64).await;
                    let bar = progress.start_file(&file.path, file.size.max(0) as u64);
                    let outcome = self.sync_one(&file, progress, bar).await;
                    progress.finish_file(bar);
                    let processed = done.fetch_add(1, Ordering::Relaxed) + 1;
                    match &outcome {
                        Ok(()) => trace!(path = %file.path, "已同步"),
                        Err(e) => {
                            has_error.store(true, Ordering::Relaxed);
                            error!(error = %e, path = %file.path, "下载文件失败");
                        }
                    }
                    // With the bars on screen a line per file would shred them;
                    // the fallback keeps a piped pass observable.
                    if !progress.is_live() && (processed % 100 == 0 || processed == total) {
                        info!(processed, total, "同步进度");
                    }
                    outcome
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;
        drop(results);
        progress.finish();

        if has_error.load(Ordering::Relaxed) {
            Err(Error::Other("sync failed".into()))
        } else {
            info!("同步完成");
            Ok(())
        }
    }

    /// Refuse a sync that would not fit on the cache filesystem.
    ///
    /// Filling the disk breaks the agent in ways that are hard to diagnose, so
    /// it is better to fail the pass with a clear message than to half-fill it.
    async fn check_free_space(&self, missing: &[FileInfo]) -> Result<()> {
        let cache_dir = self.config.cache_dir();
        // The probe needs a path that exists. The file backend creates this in
        // `check`, but a remote backend may never have touched it.
        let _ = tokio::fs::create_dir_all(&cache_dir).await;

        let needed: u64 = missing.iter().map(|file| file.size.max(0) as u64).sum();
        // Ten percent of headroom covers staging files and filesystem metadata.
        let required = needed + needed / 10;
        let usage = match crate::disk::usage(&cache_dir) {
            Ok(usage) => usage,
            Err(e) => {
                // A filesystem we cannot probe is not a reason to refuse work.
                debug!(error = %e, "读不到剩余空间，跳过检查");
                return Ok(());
            }
        };
        if usage.free < required {
            return Err(Error::storage(format!(
                "剩余空间不足：可用 {} MiB，约需 {} MiB",
                usage.free / (1024 * 1024),
                required / (1024 * 1024)
            )));
        }
        if usage.free_ratio() < LOW_SPACE_RATIO {
            warn!(
                free_mib = usage.free / (1024 * 1024),
                free_percent = (usage.free_ratio() * 100.0).round(),
                "缓存所在文件系统空间不足"
            );
        }
        Ok(())
    }

    async fn sync_one(&self, file: &FileInfo, progress: &Progress, bar: FileBar) -> Result<()> {
        let mut attempt = 0u32;
        loop {
            match self.download_and_store(file, progress, bar).await {
                Ok(()) => return Ok(()),
                Err(failure) => {
                    attempt += 1;
                    // The master wants to know when a download ends up on a
                    // different URL than the one it handed out.
                    if let Some(final_url) = failure.redirect.clone() {
                        self.report_redirect(file, &failure.error, final_url).await;
                    }
                    if attempt > SYNC_RETRIES {
                        return Err(failure.error);
                    }
                    debug!(
                        error = %failure.error,
                        path = %file.path,
                        attempt,
                        "下载失败，正在重试"
                    );
                    tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await;
                }
            }
        }
    }

    /// Tell the master that `file` was served from `final_url` instead of the
    /// URL it advertised.
    async fn report_redirect(&self, file: &FileInfo, error: &Error, final_url: String) {
        let requested = format!(
            "{}/{}",
            self.client.base(),
            file.path.trim_start_matches('/')
        );
        let payload = json!({
            "urls": [requested, final_url],
            "error": serde_json::to_string(&json!({ "message": error.to_string() })).unwrap_or_default(),
        });
        if let Err(e) = self.client.report(payload).await {
            error!(error = %e, "上报重定向失败");
        }
    }
}
