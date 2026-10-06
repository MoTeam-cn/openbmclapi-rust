//! `WebdavStorage`: the `Storage` trait implementation on top of the client.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use async_trait::async_trait;
use axum::response::Response;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{info, trace, warn};

use crate::error::{Error, Result};
use crate::storage::{ServeRequest, ServeStat, Storage};
use crate::types::{FileInfo, GcCounter};
use crate::util::get_size;

use super::cache::TtlCache;
use super::client::WebdavClient;
use super::response::{empty_ok, redirect};

/// WebDAV-backed storage.
pub struct WebdavStorage {
    client: WebdavClient,
    base_path: String,
    files: Mutex<HashMap<String, (i64, String)>>,
    empty_files: Mutex<HashSet<String>>,
    exists_cache: TtlCache,
}

impl WebdavStorage {
    /// Build from the JSON options: `url` plus optional `basePath`, `username`, `password`.
    pub fn new(opts: &Value) -> Result<Self> {
        WebdavStorage::with_label("webdav", opts)
    }

    /// The same, reporting errors under `label`.
    ///
    /// AList is this backend with a different link resolver, so a missing option
    /// there must not blame WebDAV.
    pub fn with_label(label: &str, opts: &Value) -> Result<Self> {
        let url = string_field(opts, "url")
            .ok_or_else(|| Error::Config(format!("{label}：必须提供 url")))?;
        // Caught here rather than on the first request: a malformed url is a
        // configuration mistake, and the supervisor must not loop on it.
        if url::Url::parse(&url).is_err() {
            return Err(Error::Config(format!(
                "{label}：url {url:?} 不是合法的绝对地址"
            )));
        }
        let base_path = string_field(opts, "basePath").unwrap_or_default();
        let client = WebdavClient::new(
            &url,
            string_field(opts, "username"),
            string_field(opts, "password"),
        )?;
        Ok(WebdavStorage {
            client,
            base_path,
            files: Mutex::new(HashMap::new()),
            empty_files: Mutex::new(HashSet::new()),
            exists_cache: TtlCache::new(Duration::from_secs(3600)),
        })
    }

    pub(crate) fn remote(&self, key: &str) -> String {
        self.client.url(&format!(
            "{}/{}",
            self.base_path.trim_matches('/'),
            key.trim_start_matches('/')
        ))
    }

    /// Size of an object the agent has already written, if known.
    pub(crate) async fn known_size(&self, hash: &str) -> Option<i64> {
        self.files.lock().await.get(hash).map(|(size, _)| *size)
    }

    /// Whether the object was stored as a zero-byte placeholder.
    pub(crate) async fn is_empty_file(&self, key: &str) -> bool {
        self.empty_files.lock().await.contains(key)
    }

    /// Underlying WebDAV client.
    pub(crate) fn client(&self) -> &WebdavClient {
        &self.client
    }
}

#[async_trait]
impl Storage for WebdavStorage {
    async fn init(&self) -> Result<()> {
        let root = self.remote("");
        if !self.client.exists(&root).await? {
            info!(path = %self.base_path, "创建 WebDAV 根目录");
            self.client.create_directory(&root).await?;
        }
        Ok(())
    }

    async fn check(&self) -> Result<bool> {
        let probe = self.remote(".check");
        let outcome = self.client.put(&probe, now_string().as_bytes()).await;
        let _ = self.client.delete(&probe).await;
        match outcome {
            Ok(()) => Ok(true),
            Err(e) => {
                warn!(error = %e, "存储检查失败");
                Ok(false)
            }
        }
    }

    async fn write_file(&self, path: &str, content: &[u8], file_info: &FileInfo) -> Result<()> {
        if content.is_empty() {
            self.empty_files.lock().await.insert(path.to_string());
            return Ok(());
        }
        let url = self.remote(path);
        self.client.put(&url, content).await?;
        self.files.lock().await.insert(
            file_info.hash.clone(),
            (content.len() as i64, file_info.path.clone()),
        );
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        if self.exists_cache.contains(path).await {
            return Ok(true);
        }
        let found = self.client.exists(&self.remote(path)).await?;
        if found {
            self.exists_cache.insert(path).await;
        }
        Ok(found)
    }

    async fn get_missing_files(&self, files: &[FileInfo]) -> Result<Vec<FileInfo>> {
        let mut remote: HashMap<String, FileInfo> =
            files.iter().map(|f| (f.hash.clone(), f.clone())).collect();

        {
            let known = self.files.lock().await;
            if !known.is_empty() {
                for hash in known.keys() {
                    remote.remove(hash);
                }
                return Ok(remote.into_values().collect());
            }
        }

        let root = self.remote("");
        let mut queue = vec![root];
        let mut checked = 0usize;
        while let Some(dir) = queue.pop() {
            let entries = self.client.propfind(&dir, 1).await?;
            checked += 1;
            trace!(dir = %dir, checked, "正在扫描 WebDAV 目录");
            for entry in entries {
                if entry.is_dir {
                    // The reserved probe folder is not part of the master's list.
                    if entry.name == crate::storage::measure::DIR {
                        continue;
                    }
                    queue.push(entry.href.clone());
                    continue;
                }
                if let Some(file) = remote.get(&entry.name) {
                    if file.size == entry.size {
                        self.files
                            .lock()
                            .await
                            .insert(entry.name.clone(), (entry.size, entry.href.clone()));
                        remote.remove(&entry.name);
                    }
                }
            }
        }
        Ok(remote.into_values().collect())
    }

    async fn gc(&self, files: &[FileInfo]) -> Result<GcCounter> {
        let wanted: HashSet<String> = files.iter().map(|f| f.hash.clone()).collect();
        let mut counter = GcCounter::default();
        let root = self.remote("");
        let mut queue = vec![root];
        while let Some(dir) = queue.pop() {
            let entries = self.client.propfind(&dir, 1).await?;
            for entry in entries {
                if entry.is_dir {
                    queue.push(entry.href.clone());
                    continue;
                }
                if !wanted.contains(&entry.name) {
                    info!(path = %entry.href, "删除过期文件");
                    if self.client.delete(&entry.href).await.is_ok() {
                        self.files.lock().await.remove(&entry.name);
                        self.exists_cache.remove(&entry.name).await;
                        counter.count += 1;
                        counter.size += entry.size.max(0) as u64;
                    }
                }
            }
        }
        Ok(counter)
    }

    async fn serve(&self, req: ServeRequest<'_>) -> Result<(Response, ServeStat)> {
        if self.is_empty_file(req.hash_path).await {
            return Ok((empty_ok(), ServeStat { bytes: 0, hits: 1 }));
        }
        let url = self.client.download_link(&self.remote(req.hash_path));
        let size = get_size(self.known_size(req.hash).await.unwrap_or(0), req.range);
        Ok((
            redirect(url),
            ServeStat {
                bytes: size.max(0) as u64,
                hits: 1,
            },
        ))
    }
}

fn string_field(opts: &Value, key: &str) -> Option<String> {
    opts.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn now_string() -> String {
    crate::util::now_ms().to_string()
}

#[cfg(test)]
#[path = "storage_test.rs"]
mod tests;
