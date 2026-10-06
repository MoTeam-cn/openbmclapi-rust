//! File synchronisation and on-demand object downloads.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace};

use crate::error::{Error, Result};
use crate::types::{FileInfo, FileList, SyncConfig};
use crate::util::{hash_to_filename, now_ms};

use super::checksum::validate_file;
use super::cluster::Cluster;

/// Retries per file when syncing, matching `p-retry` in the Node agent.
const SYNC_RETRIES: u32 = 10;

impl Cluster {
    /// Download every file that is missing or size-mismatched.
    pub async fn sync_files(&self, file_list: &FileList, sync: &SyncConfig) -> Result<()> {
        if !self.storage.check().await? {
            return Err(Error::storage("storage is not writable"));
        }
        info!("checking for missing files");
        let missing = self.storage.get_missing_files(&file_list.files).await?;
        if missing.is_empty() {
            return Ok(());
        }
        info!(count = missing.len(), "mismatch found, starting sync");
        info!(concurrency = sync.concurrency, "sync strategy");

        let concurrency = sync.concurrency.max(1);
        let total = missing.len();
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let has_error = Arc::new(AtomicBool::new(false));

        let results = futures::stream::iter(missing.into_iter())
            .map(|file| {
                let done = Arc::clone(&done);
                let has_error = Arc::clone(&has_error);
                async move {
                    let outcome = self.sync_one(&file).await;
                    let processed = done.fetch_add(1, Ordering::Relaxed) + 1;
                    match &outcome {
                        Ok(()) => trace!(path = %file.path, "synced"),
                        Err(e) => {
                            has_error.store(true, Ordering::Relaxed);
                            error!(error = %e, path = %file.path, "failed to download file");
                        }
                    }
                    if processed % 100 == 0 || processed == total {
                        info!(processed, total, "sync progress");
                    }
                    outcome
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;
        drop(results);

        if has_error.load(Ordering::Relaxed) {
            Err(Error::Other("sync failed".into()))
        } else {
            info!("sync complete");
            Ok(())
        }
    }

    async fn sync_one(&self, file: &FileInfo) -> Result<()> {
        let mut attempt = 0u32;
        loop {
            match self.download_and_store(file).await {
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
                        "download failed, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await;
                }
            }
        }
    }

    async fn download_and_store(
        &self,
        file: &FileInfo,
    ) -> std::result::Result<(), DownloadFailure> {
        let path = file.path.trim_start_matches('/');
        let requested = format!("{}/{}", self.client.base(), path);
        let response = self
            .client
            .get_stream(path, &[])
            .await
            .map_err(DownloadFailure::new)?;

        // `reqwest` follows redirects; the final URL reveals whether the object
        // actually came from the master or from a CDN.
        let final_url = response.url().to_string();
        let redirect = (final_url != requested).then_some(final_url);

        let status = response.status();
        if !status.is_success() {
            return Err(DownloadFailure {
                error: Error::Status {
                    status: status.as_u16(),
                    url: file.path.clone(),
                },
                redirect,
            });
        }

        let mut body = Vec::with_capacity(file.size.max(0) as usize);
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| DownloadFailure::new(e.into()))?
        {
            body.extend_from_slice(&chunk);
        }
        if !validate_file(&body, &file.hash) {
            return Err(DownloadFailure {
                error: Error::Other(format!("checksum mismatch for {}", file.path)),
                redirect,
            });
        }
        self.storage
            .write_file(&hash_to_filename(&file.hash), &body, file)
            .await
            .map_err(|error| DownloadFailure { error, redirect })
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
            error!(error = %e, "failed to report redirect");
        }
    }

    /// Ensure an object is present locally, de-duplicating concurrent requests.
    pub async fn ensure_downloaded(&self, hash: &str) -> Result<()> {
        let hash_path = hash_to_filename(hash);
        if self.storage.exists(&hash_path).await? {
            return Ok(());
        }

        let (lock, fresh) = {
            let mut locks = self.download_locks.lock().await;
            match locks.get(hash) {
                Some(existing) => (Arc::clone(existing), false),
                None => {
                    let created = Arc::new(Mutex::new(()));
                    locks.insert(hash.to_string(), Arc::clone(&created));
                    (created, true)
                }
            }
        };

        let guard = lock.lock().await;
        if self.storage.exists(&hash_path).await? {
            drop(guard);
            if fresh {
                self.download_locks.lock().await.remove(hash);
            }
            return Ok(());
        }
        let result = self.download_file(hash).await;
        drop(guard);
        if fresh {
            self.download_locks.lock().await.remove(hash);
        }
        result
    }

    /// Fetch a single object from the master on demand.
    pub async fn download_file(&self, hash: &str) -> Result<()> {
        let response = self
            .client
            .get_stream(
                &format!("openbmclapi/download/{hash}"),
                &[("noopen", "1".to_string())],
            )
            .await?;
        let status = response.status();
        if status.as_u16() == 404 {
            return Err(Error::NotFound);
        }
        if !status.is_success() {
            return Err(Error::Status {
                status: status.as_u16(),
                url: format!("openbmclapi/download/{hash}"),
            });
        }
        let body = response.bytes().await?;
        let info = FileInfo {
            path: format!("/download/{hash}"),
            hash: hash.to_string(),
            size: body.len() as i64,
            mtime: now_ms(),
        };
        self.storage
            .write_file(&hash_to_filename(hash), &body, &info)
            .await
    }
}

/// A failed file download, carrying the redirect target when there was one.
struct DownloadFailure {
    error: Error,
    redirect: Option<String>,
}

impl DownloadFailure {
    fn new(error: Error) -> Self {
        DownloadFailure {
            error,
            redirect: None,
        }
    }
}
