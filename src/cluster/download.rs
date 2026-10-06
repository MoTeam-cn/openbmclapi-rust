//! Fetching one object from the master and storing it.
//!
//! The body is buffered whole so its checksum can be verified before anything
//! is written; the byte accounting goes to the progress display as the chunks
//! arrive.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::logger::progress::{FileBar, Progress};
use crate::types::FileInfo;
use crate::util::{hash_to_filename, now_ms};

use super::checksum::validate_file;
use super::cluster::Cluster;

impl Cluster {
    pub(super) async fn download_and_store(
        &self,
        file: &FileInfo,
        progress: &Progress,
        bar: FileBar,
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
            progress.add_bytes(bar, chunk.len() as u64);
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
pub(super) struct DownloadFailure {
    pub(super) error: Error,
    pub(super) redirect: Option<String>,
}

impl DownloadFailure {
    pub(super) fn new(error: Error) -> Self {
        DownloadFailure {
            error,
            redirect: None,
        }
    }
}
