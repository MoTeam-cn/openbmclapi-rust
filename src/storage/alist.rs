//! AList-flavoured WebDAV storage.
//!
//! AList hands out short-lived signed download links, so the agent resolves the
//! link once, caches it, and afterwards either replays the cached redirect or
//! proxies the upstream body. Mirrors `alist-webdav.storage.ts`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::header;
use axum::response::Response;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::error::{Error, Result};
use crate::types::{FileInfo, GcCounter};
use crate::util::{get_size, now_ms};

use crate::storage::resilience::{ResilienceConfig, ResilientClient};
use crate::storage::shared::copy_passthrough;

use super::webdav::{empty_ok, redirect, WebdavStorage};
use super::{ServeRequest, ServeStat, Storage};

/// Default redirect cache TTL, matching the Node agent.
const DEFAULT_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    url: String,
    expires_at_ms: i64,
}

/// TTL cache of signed download links, persisted next to the file cache.
struct RedirectCache {
    path: PathBuf,
    ttl: Duration,
    entries: Mutex<HashMap<String, CacheEntry>>,
}

impl RedirectCache {
    async fn load(path: PathBuf, ttl: Duration) -> Self {
        let entries = match tokio::fs::read(&path).await {
            Ok(bytes) => {
                serde_json::from_slice::<HashMap<String, CacheEntry>>(&bytes).unwrap_or_default()
            }
            Err(_) => HashMap::new(),
        };
        let now = now_ms();
        let live: HashMap<String, CacheEntry> = entries
            .into_iter()
            .filter(|(_, entry)| entry.expires_at_ms > now)
            .collect();
        RedirectCache {
            path,
            ttl,
            entries: Mutex::new(live),
        }
    }

    async fn get(&self, key: &str) -> Option<String> {
        let guard = self.entries.lock().await;
        guard
            .get(key)
            .filter(|entry| entry.expires_at_ms > now_ms())
            .map(|entry| entry.url.clone())
    }

    async fn set(&self, key: &str, url: String) {
        let mut guard = self.entries.lock().await;
        guard.insert(
            key.to_string(),
            CacheEntry {
                url,
                expires_at_ms: now_ms() + self.ttl.as_millis() as i64,
            },
        );
        let snapshot = guard.clone();
        drop(guard);
        if let Some(parent) = self.path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        match serde_json::to_vec(&snapshot) {
            Ok(bytes) => {
                if let Err(e) = tokio::fs::write(&self.path, bytes).await {
                    debug!(error = %e, "failed to persist redirect url cache");
                }
            }
            Err(e) => debug!(error = %e, "failed to serialise redirect url cache"),
        }
    }
}

/// AList storage: WebDAV plus signed-link handling.
pub struct AlistStorage {
    inner: WebdavStorage,
    cache: RedirectCache,
    http: ResilientClient,
}

impl AlistStorage {
    pub fn new(opts: &Value) -> Result<Self> {
        let inner = WebdavStorage::new(opts)?;
        let ttl = match opts.get("cacheTtl") {
            Some(Value::Number(n)) => Duration::from_millis(n.as_u64().unwrap_or(3_600_000)),
            Some(Value::String(s)) => parse_duration(s).unwrap_or(DEFAULT_TTL),
            _ => DEFAULT_TTL,
        };
        let cache_path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("cache")
            .join("redirectUrl.json");
        let raw = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            // The signed link is expected to answer 302; the agent replays that
            // to its own client rather than following it here.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(16)
            .pool_idle_timeout(Duration::from_secs(60))
            .tcp_keepalive(Duration::from_secs(30))
            .tcp_nodelay(true)
            .build()?;
        let http = ResilientClient::new(raw, ResilienceConfig::default());
        Ok(AlistStorage {
            inner,
            cache: RedirectCache {
                path: cache_path,
                ttl,
                entries: Mutex::new(HashMap::new()),
            },
            http,
        })
    }

    /// Replace the in-memory redirect cache with the persisted one.
    async fn load_cache(&self) {
        let loaded = RedirectCache::load(self.cache.path.clone(), self.cache.ttl).await;
        *self.cache.entries.lock().await = loaded.entries.into_inner();
    }
}

#[async_trait]
impl Storage for AlistStorage {
    async fn init(&self) -> Result<()> {
        self.inner.init().await?;
        self.load_cache().await;
        Ok(())
    }

    async fn check(&self) -> Result<bool> {
        self.inner.check().await
    }

    async fn write_file(&self, path: &str, content: &[u8], file_info: &FileInfo) -> Result<()> {
        self.inner.write_file(path, content, file_info).await
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        self.inner.exists(path).await
    }

    async fn get_missing_files(&self, files: &[FileInfo]) -> Result<Vec<FileInfo>> {
        self.inner.get_missing_files(files).await
    }

    async fn gc(&self, files: &[FileInfo]) -> Result<GcCounter> {
        self.inner.gc(files).await
    }

    async fn serve(&self, req: ServeRequest<'_>) -> Result<(Response, ServeStat)> {
        if self.inner.is_empty_file(req.hash_path).await {
            return Ok((empty_ok(), ServeStat { bytes: 0, hits: 1 }));
        }
        let size = get_size(
            self.inner.known_size(req.hash).await.unwrap_or(0),
            req.range,
        );

        if let Some(cached) = self.cache.get(req.hash_path).await {
            return Ok((
                redirect(cached),
                ServeStat {
                    bytes: size.max(0) as u64,
                    hits: 1,
                },
            ));
        }

        let url = self
            .inner
            .client()
            .download_link(&self.inner.remote(req.hash_path));
        let response = self
            .http
            .send(&url, || {
                let builder = self.http.raw().get(&url);
                match req.range {
                    Some(range) => builder.header(header::RANGE, range),
                    None => builder,
                }
            })
            .await?;
        let status = response.status();

        if status.is_success() {
            // Stream rather than buffer: a mirror object can be hundreds of
            // megabytes, and buffering it is what used to kill the agent.
            let bytes = response.content_length().unwrap_or(size.max(0) as u64);
            let builder = copy_passthrough(&response, Response::builder().status(status));
            let body = Body::from_stream(
                response
                    .bytes_stream()
                    .map(|result| result.map_err(std::io::Error::other)),
            );
            let reply = builder
                .body(body)
                .map_err(|e| Error::Other(format!("failed to build response: {e}")))?;
            return Ok((reply, ServeStat { bytes, hits: 1 }));
        }

        if status.is_redirection() {
            if let Some(location) = response.headers().get(header::LOCATION).cloned() {
                let target = location.to_str().unwrap_or_default().to_string();
                self.cache.set(req.hash_path, target.clone()).await;
                let reply = Response::builder()
                    .status(status)
                    .header(header::LOCATION, target)
                    .body(Body::empty())
                    .map_err(|e| Error::Other(format!("failed to build response: {e}")))?;
                return Ok((
                    reply,
                    ServeStat {
                        bytes: size.max(0) as u64,
                        hits: 1,
                    },
                ));
            }
        }

        warn!(status = status.as_u16(), "alist download failed");
        Err(Error::Status {
            status: status.as_u16(),
            url,
        })
    }
}

/// Parse a duration such as `1h`, `30m`, `45s`, `2d`.
pub fn parse_duration(input: &str) -> Option<Duration> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let split = input
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(input.len());
    let (value, unit) = input.split_at(split);
    let value: u64 = value.parse().ok()?;
    let multiplier = match unit.trim() {
        "ms" => return Some(Duration::from_millis(value)),
        "" | "s" | "sec" | "secs" => 1000,
        "m" | "min" | "mins" => 60 * 1000,
        "h" | "hr" | "hrs" => 60 * 60 * 1000,
        "d" | "day" | "days" => 24 * 60 * 60 * 1000,
        _ => return None,
    };
    Some(Duration::from_millis(value * multiplier))
}

#[cfg(test)]
#[path = "alist_test.rs"]
mod tests;
