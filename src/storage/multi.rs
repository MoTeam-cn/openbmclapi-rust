//! A pool of remote storage backends.
//!
//! Downloads rotate across the pool so no single upstream carries every
//! request, and a source that fails falls through to the next one. Writes are
//! replicated to every source, so any of them can serve the object later.
//!
//! The local `file` backend is deliberately not allowed in a pool: it is the
//! agent's own cache, not a remote mirror, so mixing it in would give the pool
//! two incompatible notions of where the data lives.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::response::Response;
use tracing::{info, warn};

use crate::error::{Error, Result};
use crate::types::{FileInfo, GcCounter};

use super::backend::{ServeRequest, ServeStat, Storage};

/// Spreads reads across several backends and replicates writes to all of them.
pub struct MultiStorage {
    sources: Vec<Arc<dyn Storage>>,
    cursor: AtomicUsize,
}

impl MultiStorage {
    /// Build a pool. Two sources is the minimum that makes a pool meaningful.
    pub fn new(sources: Vec<Arc<dyn Storage>>) -> Result<Self> {
        if sources.len() < 2 {
            return Err(Error::Config(
                "a storage pool needs at least two sources".into(),
            ));
        }
        Ok(MultiStorage {
            sources,
            cursor: AtomicUsize::new(0),
        })
    }

    /// How many backends are in the pool.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Round-robin, so the spread is guaranteed rather than merely likely.
    fn next_index(&self) -> usize {
        self.cursor.fetch_add(1, Ordering::Relaxed) % self.sources.len()
    }
}

#[async_trait]
impl Storage for MultiStorage {
    async fn init(&self) -> Result<()> {
        for (index, source) in self.sources.iter().enumerate() {
            source
                .init()
                .await
                .map_err(|e| Error::storage(format!("source {index} failed to initialise: {e}")))?;
        }
        Ok(())
    }

    async fn check(&self) -> Result<bool> {
        for source in &self.sources {
            if !source.check().await? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn write_file(&self, path: &str, content: &[u8], file_info: &FileInfo) -> Result<()> {
        let mut failures = 0usize;
        let mut last: Option<Error> = None;
        for (index, source) in self.sources.iter().enumerate() {
            if let Err(e) = source.write_file(path, content, file_info).await {
                // Best effort: a source that missed the write simply reports the
                // object as missing on the next sync pass and gets it then.
                warn!(source = index, path, error = %e, "replication failed");
                failures += 1;
                last = Some(e);
            }
        }
        if failures == self.sources.len() {
            return Err(last.unwrap_or_else(|| Error::storage("every source rejected the write")));
        }
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        for source in &self.sources {
            if !source.exists(path).await? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn get_missing_files(&self, files: &[FileInfo]) -> Result<Vec<FileInfo>> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut missing: Vec<FileInfo> = Vec::new();
        for source in &self.sources {
            for file in source.get_missing_files(files).await? {
                if seen.insert(file.path.clone()) {
                    missing.push(file);
                }
            }
        }
        Ok(missing)
    }

    async fn gc(&self, files: &[FileInfo]) -> Result<GcCounter> {
        let mut total = GcCounter::default();
        for source in &self.sources {
            let counter = source.gc(files).await?;
            total.count += counter.count;
            total.size += counter.size;
        }
        Ok(total)
    }

    async fn serve(&self, req: ServeRequest<'_>) -> Result<(Response, ServeStat)> {
        let start = self.next_index();
        let mut last: Option<Error> = None;
        for offset in 0..self.sources.len() {
            let index = (start + offset) % self.sources.len();
            match self.sources[index].serve(req).await {
                Ok(ok) => {
                    if offset > 0 {
                        info!(source = index, offset, "served from a fallback source");
                    }
                    return Ok(ok);
                }
                Err(Error::NotFound) => {}
                Err(e) => {
                    warn!(source = index, error = %e, "source failed, trying the next one");
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or(Error::NotFound))
    }
}

#[cfg(test)]
#[path = "multi_test.rs"]
mod tests;
