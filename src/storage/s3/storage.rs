//! `MinioStorage`: construction and the `Storage` trait implementation.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use reqwest::Client;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::error::{Error, Result};
use crate::storage::{ServeRequest, ServeStat, Storage};
use crate::types::{FileInfo, GcCounter};
use crate::util::{basename, get_size, now_ms};

use super::endpoint::{split_bucket_prefix, Endpoint};
use super::sigv4::presign_get;
use crate::storage::shared::{encode_filename, encode_path, join_object_key, strip_prefix_key};

/// How long a positive `exists` result stays cached.
const EXISTS_TTL: Duration = Duration::from_secs(3600);

/// Cached metadata for an object known to be present.
#[derive(Debug, Clone)]
pub(super) struct FileEntry {
    size: i64,
    path: String,
}

/// MinIO / S3-compatible storage backend.
pub struct MinioStorage {
    pub(super) http: Client,
    pub(super) public: Endpoint,
    pub(super) internal: Endpoint,
    pub(super) bucket: String,
    pub(super) prefix: String,
    pub(super) files: Mutex<HashMap<String, FileEntry>>,
    pub(super) exists_cache: Mutex<HashMap<String, Instant>>,
}

impl MinioStorage {
    /// Build a backend from the `CLUSTER_STORAGE_OPTIONS` JSON object.
    pub fn new(opts: &Value) -> Result<Self> {
        let raw = opts
            .get("url")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::Config("minio storage requires a non-empty \"url\"".into()))?;
        let public = Endpoint::parse(raw, None)?;
        let internal = match opts.get("internalUrl").and_then(Value::as_str) {
            Some(value) if !value.is_empty() => Endpoint::parse(value, Some(&public.region))?,
            _ => public.clone(),
        };
        let (bucket, prefix) = split_bucket_prefix(raw)?;
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(16)
            .tcp_nodelay(true)
            .build()?;
        Ok(MinioStorage {
            http,
            public,
            internal,
            bucket,
            prefix,
            files: Mutex::new(HashMap::new()),
            exists_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Full object key for a storage-relative path.
    fn object_key(&self, path: &str) -> String {
        join_object_key(&self.prefix, path)
    }

    /// Canonical URI of an object.
    pub(super) fn object_uri(&self, key: &str) -> String {
        format!("/{}/{}", self.bucket, encode_path(key))
    }

    /// Canonical URI of the bucket, used for listings.
    pub(super) fn bucket_uri(&self) -> String {
        format!("/{}", self.bucket)
    }
}

#[async_trait]
impl Storage for MinioStorage {
    async fn check(&self) -> Result<bool> {
        let key = self.object_key(".check");
        let body = Bytes::from(now_ms().to_string());
        let outcome = async {
            self.put_object(&self.internal, &key, body.clone()).await?;
            self.put_object(&self.public, &key, body.clone()).await?;
            Ok::<(), Error>(())
        }
        .await;
        if let Err(err) = self.delete_object(&self.internal, &key).await {
            warn!(%err, "failed to delete temp file");
        }
        if let Err(err) = self.delete_object(&self.public, &key).await {
            warn!(%err, "failed to delete temp file");
        }
        match outcome {
            Ok(()) => Ok(true),
            Err(err) => {
                error!(%err, "storage check failed");
                Ok(false)
            }
        }
    }

    async fn write_file(&self, path: &str, content: &[u8], file_info: &FileInfo) -> Result<()> {
        let key = self.object_key(path);
        self.put_object(&self.internal, &key, Bytes::copy_from_slice(content))
            .await?;
        self.files.lock().await.insert(
            file_info.hash.clone(),
            FileEntry {
                size: file_info.size,
                path: file_info.path.clone(),
            },
        );
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        if let Some(seen) = self.exists_cache.lock().await.get(path) {
            if seen.elapsed() < EXISTS_TTL {
                return Ok(true);
            }
        }
        let key = self.object_key(path);
        if self.head_object(&self.internal, &key).await? {
            self.exists_cache
                .lock()
                .await
                .insert(path.to_string(), Instant::now());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn get_missing_files(&self, files: &[FileInfo]) -> Result<Vec<FileInfo>> {
        let mut wanted: HashMap<String, &FileInfo> =
            files.iter().map(|file| (file.hash.clone(), file)).collect();
        let known: Vec<String> = {
            let guard = self.files.lock().await;
            if guard.is_empty() {
                Vec::new()
            } else {
                guard.keys().cloned().collect()
            }
        };
        if known.is_empty() {
            for object in self.list_all(&self.internal).await? {
                let hash = basename(&object.key).to_string();
                let matches = wanted
                    .get(&hash)
                    .map(|file| file.size == object.size)
                    .unwrap_or(false);
                if matches {
                    self.files.lock().await.insert(
                        hash.clone(),
                        FileEntry {
                            size: object.size,
                            path: strip_prefix_key(&object.key, &self.prefix).to_string(),
                        },
                    );
                    wanted.remove(&hash);
                }
            }
        } else {
            for hash in known {
                wanted.remove(&hash);
            }
        }
        Ok(files
            .iter()
            .filter(|file| wanted.contains_key(&file.hash))
            .cloned()
            .collect())
    }

    async fn gc(&self, files: &[FileInfo]) -> Result<GcCounter> {
        let wanted: HashSet<String> = files.iter().map(|file| file.hash.clone()).collect();
        let mut counter = GcCounter::default();
        for object in self.list_all(&self.internal).await? {
            let hash = basename(&object.key);
            if wanted.contains(hash) {
                continue;
            }
            info!(path = %object.key, "delete expire file");
            self.delete_object(&self.internal, &object.key).await?;
            self.files.lock().await.remove(hash);
            counter.count += 1;
            counter.size += object.size.max(0) as u64;
        }
        Ok(counter)
    }

    async fn serve(&self, req: ServeRequest<'_>) -> Result<(Response, ServeStat)> {
        let key = self.object_key(req.hash_path);
        let known = self.files.lock().await.get(req.hash).cloned();
        let filename = match req.name {
            Some(name) => Some(name.to_string()),
            None => known
                .as_ref()
                .map(|entry| basename(&entry.path).to_string()),
        };
        let disposition =
            filename.map(|name| format!("attachment; filename=\"{}\"", encode_filename(&name)));
        let location = presign_get(
            &self.public,
            &self.object_uri(&key),
            60,
            disposition.as_deref(),
        );
        let bytes = get_size(known.map(|entry| entry.size).unwrap_or(0), req.range).max(0) as u64;
        let response = Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, location.as_str())
            .body(Body::empty())
            .map_err(|err| Error::other(format!("failed to build redirect response: {err}")))?;
        Ok((response, ServeStat { bytes, hits: 1 }))
    }
}
