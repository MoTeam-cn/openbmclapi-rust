//! A pool of storage backends, of one or many.
//!
//! Downloads rotate across the pool so no single upstream carries every
//! request, and a source that fails falls through to the next one. Writes are
//! replicated to every source, so any of them can serve the object later.
//!
//! Every configuration goes through here, even a single source, because this is
//! where the bandwidth-probe policy lives. A member can opt out of handing out
//! stored probes, and the pool then either measures through one that opted in or
//! reports that there is nobody to measure through.
//!
//! The local `file` backend is never a probe host: it is the agent's own cache,
//! which is never seeded with probes in the first place.

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
    /// Per source: whether probes may be served from its own link.
    measure_redirect: Vec<bool>,
    cursor: AtomicUsize,
}

impl MultiStorage {
    /// Build a pool whose every member is a probe host.
    pub fn new(sources: Vec<Arc<dyn Storage>>) -> Result<Self> {
        let policy = vec![true; sources.len()];
        MultiStorage::with_measure_redirect(sources, policy)
    }

    /// Build a pool with an explicit per-source probe policy.
    pub fn with_measure_redirect(
        sources: Vec<Arc<dyn Storage>>,
        measure_redirect: Vec<bool>,
    ) -> Result<Self> {
        if sources.is_empty() {
            return Err(Error::Config("存储池至少需要一个源".into()));
        }
        if sources.len() != measure_redirect.len() {
            return Err(Error::Config("测速策略需要与源数量一一对应".into()));
        }
        Ok(MultiStorage {
            sources,
            measure_redirect,
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

    /// Sources that agreed to answer bandwidth probes.
    fn probe_hosts(&self) -> impl Iterator<Item = usize> + '_ {
        self.measure_redirect
            .iter()
            .enumerate()
            .filter(|(_, &on)| on)
            .map(|(index, _)| index)
    }
}

#[async_trait]
impl Storage for MultiStorage {
    async fn init(&self) -> Result<()> {
        for (index, source) in self.sources.iter().enumerate() {
            source
                .init()
                .await
                .map_err(|e| Error::storage(format!("源 {index} 初始化失败：{e}")))?;
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
                warn!(source = index, path, error = %e, "复制失败");
                failures += 1;
                last = Some(e);
            }
        }
        if failures == self.sources.len() {
            return Err(last.unwrap_or_else(|| Error::storage("所有源都拒绝了这次写入")));
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
                        info!(source = index, offset, "由备用源应答");
                    }
                    return Ok(ok);
                }
                Err(Error::NotFound) => {}
                Err(e) => {
                    warn!(source = index, error = %e, "该源失败，尝试下一个");
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or(Error::NotFound))
    }

    fn measure_redirect(&self) -> bool {
        self.measure_redirect.iter().any(|&on| on)
    }

    async fn serve_measure(&self, size_mib: u64) -> Result<Option<(Response, ServeStat)>> {
        let hosts: Vec<usize> = self.probe_hosts().collect();
        if hosts.is_empty() {
            return Ok(None);
        }
        // Rotate so repeated probes spread across the hosts that opted in rather
        // than always measuring the first one.
        let start = self.cursor.fetch_add(1, Ordering::Relaxed) % hosts.len();
        let mut last: Option<Error> = None;
        for offset in 0..hosts.len() {
            let index = hosts[(start + offset) % hosts.len()];
            match super::measure::serve_from(&*self.sources[index], size_mib).await {
                Ok(Some(ok)) => {
                    if offset > 0 {
                        info!(source = index, offset, "测速由备用源应答");
                    }
                    return Ok(Some(ok));
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(source = index, error = %e, "测速源失败，尝试下一个");
                    last = Some(e);
                }
            }
        }
        match last {
            Some(e) => Err(e),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
#[path = "multi_test.rs"]
mod tests;
