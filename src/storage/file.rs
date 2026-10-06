//! Local filesystem storage (the default backend).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::stream::StreamExt;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use tracing::{info, warn};

use crate::error::{Error, Result};
use crate::storage::shared::encode_filename;
use crate::types::{FileInfo, GcCounter};
use crate::util::{get_size, hash_to_filename};

use super::{join_key, ServeRequest, ServeStat, Storage};

/// Stores objects under `cache/<ab>/<hash>`.
pub struct FileStorage {
    cache_dir: PathBuf,
}

impl FileStorage {
    pub fn new(cache_dir: PathBuf) -> Self {
        FileStorage { cache_dir }
    }

    fn absolute(&self, key: &str) -> PathBuf {
        join_key(&self.cache_dir, key)
    }

    fn missing_concurrency() -> usize {
        1000
    }
}

#[async_trait]
impl Storage for FileStorage {
    async fn check(&self) -> Result<bool> {
        let probe = self.cache_dir.join(".check");
        let outcome = async {
            tokio::fs::create_dir_all(&self.cache_dir).await?;
            tokio::fs::write(&probe, b"").await?;
            Ok::<(), std::io::Error>(())
        }
        .await;
        let _ = tokio::fs::remove_file(&probe).await;
        match outcome {
            Ok(()) => Ok(true),
            Err(e) => {
                warn!(error = %e, "storage check failed");
                Ok(false)
            }
        }
    }

    async fn write_file(&self, path: &str, content: &[u8], _file_info: &FileInfo) -> Result<()> {
        let target = self.absolute(path);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        // Write beside the target and rename into place. A reader arriving
        // mid-write then either misses the object or sees all of it, never a
        // truncated body; rename is atomic within one filesystem.
        let staging = staging_path(&target);
        tokio::fs::write(&staging, content).await?;
        if let Err(e) = tokio::fs::rename(&staging, &target).await {
            let _ = tokio::fs::remove_file(&staging).await;
            return Err(e.into());
        }
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        Ok(tokio::fs::try_exists(self.absolute(path))
            .await
            .unwrap_or(false))
    }

    async fn get_missing_files(&self, files: &[FileInfo]) -> Result<Vec<FileInfo>> {
        let results = futures::stream::iter(files.iter().cloned())
            .map(|file| {
                let absolute = self.absolute(&hash_to_filename(&file.hash));
                async move {
                    let size = tokio::fs::metadata(&absolute)
                        .await
                        .map(|meta| meta.len() as i64)
                        .unwrap_or(-1);
                    if size == file.size {
                        None
                    } else {
                        Some(file)
                    }
                }
            })
            .buffer_unordered(Self::missing_concurrency())
            .filter_map(|item| async move { item })
            .collect::<Vec<_>>()
            .await;
        Ok(results)
    }

    async fn gc(&self, files: &[FileInfo]) -> Result<GcCounter> {
        let mut wanted: HashSet<String> = HashSet::with_capacity(files.len());
        for file in files {
            wanted.insert(hash_to_filename(&file.hash));
        }

        let mut counter = GcCounter::default();
        let mut queue = vec![self.cache_dir.clone()];
        while let Some(dir) = queue.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let metadata = entry.metadata().await?;
                if metadata.is_dir() {
                    queue.push(path);
                    continue;
                }
                let key = path
                    .strip_prefix(&self.cache_dir)
                    .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                // A staging file belongs to a write that is still in flight;
                // collecting it would make that write fail.
                if key.ends_with(STAGING_SUFFIX) {
                    continue;
                }
                if !wanted.contains(&key) {
                    info!(path = %path.display(), "delete expire file");
                    if tokio::fs::remove_file(&path).await.is_ok() {
                        counter.count += 1;
                        counter.size += metadata.len();
                    }
                }
            }
        }
        Ok(counter)
    }

    async fn serve(&self, req: ServeRequest<'_>) -> Result<(Response, ServeStat)> {
        let path = self.absolute(req.hash_path);
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|_| Error::NotFound)?;
        if !metadata.is_file() {
            return Err(Error::NotFound);
        }
        let total = metadata.len() as i64;

        let mut builder = Response::builder()
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CACHE_CONTROL, "public, max-age=2592000");
        let mime = mime_guess::from_path(&path).first_or_octet_stream();
        builder = builder.header(header::CONTENT_TYPE, mime.as_ref());
        if let Some(name) = req.name {
            builder = builder.header(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", encode_filename(name)),
            );
        }

        let (start, end) = match req.range.and_then(|r| parse_single_range(total, r)) {
            Some((start, end)) => (start, end),
            None => (0, total.saturating_sub(1)),
        };
        let length = if total == 0 { 0 } else { end - start + 1 };
        let partial = req.range.is_some() && total > 0 && (start != 0 || length != total);

        let mut file = tokio::fs::File::open(&path).await?;
        if start > 0 {
            file.seek(std::io::SeekFrom::Start(start as u64)).await?;
        }
        let stream = ReaderStream::new(file.take(length.max(0) as u64));

        let response = if partial {
            builder
                .status(StatusCode::PARTIAL_CONTENT)
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{total}"),
                )
                .header(header::CONTENT_LENGTH, length.to_string())
                .body(Body::from_stream(stream))
        } else {
            builder
                .status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, total.to_string())
                .body(Body::from_stream(stream))
        }
        .map_err(|e| Error::Other(format!("failed to build response: {e}")))?;

        let bytes = if total == 0 { 0 } else { length as u64 };
        let _ = get_size(total, req.range);
        Ok((response, ServeStat { bytes, hits: 1 }))
    }
}

/// Suffix marking an object that is still being written.
const STAGING_SUFFIX: &str = ".part";

/// A sibling path to fill before renaming onto the final name.
///
/// The pid and a counter keep concurrent writers apart, and staying in the same
/// directory keeps the rename on one filesystem.
fn staging_path(target: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "object".to_string());
    target.with_file_name(format!("{name}.{}.{serial}.part", std::process::id()))
}

/// Parse a single-range `Range` header, ignoring multi-range requests.
fn parse_single_range(total: i64, header: &str) -> Option<(i64, i64)> {
    if total <= 0 {
        return None;
    }
    let spec = header.strip_prefix("bytes=")?;
    let spec = spec.trim();
    if spec.contains(',') {
        return None;
    }
    let (raw_start, raw_end) = spec.split_once('-')?;
    if raw_start.is_empty() {
        let suffix: i64 = raw_end.trim().parse().ok()?;
        if suffix == 0 {
            return None;
        }
        return Some(((total - suffix).max(0), total - 1));
    }
    let start: i64 = raw_start.trim().parse().ok()?;
    if start >= total {
        return None;
    }
    let end = if raw_end.trim().is_empty() {
        total - 1
    } else {
        raw_end.trim().parse::<i64>().ok()?.min(total - 1)
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

impl IntoResponse for FileStorage {
    fn into_response(self) -> Response {
        StatusCode::NOT_IMPLEMENTED.into_response()
    }
}

#[cfg(test)]
#[path = "file_test.rs"]
mod tests;
