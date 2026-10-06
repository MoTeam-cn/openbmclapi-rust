//! `OssStorage`: backend state and its `Storage` implementation.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use chrono::Utc;
use futures::StreamExt;
use reqwest::Client;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::error::{Error, Result};
use crate::storage::shared::{
    copy_passthrough, encode_component, encode_filename, encode_path, join_object_key,
    strip_prefix_key,
};
use crate::storage::{ServeRequest, ServeStat, Storage};
use crate::types::{FileInfo, GcCounter};
use crate::util::{basename, get_size, now_ms};

use super::config::{build_base_url, OssConfig};
use super::signature::{
    canonicalized_resource, hmac_sha1_base64, http_date, string_to_sign_presign,
};

/// How long a positive `exists` result stays cached.
const EXISTS_TTL: Duration = Duration::from_secs(3600);

/// Cached metadata for an object known to be present.
#[derive(Debug, Clone)]
struct FileEntry {
    size: i64,
    path: String,
}

/// Aliyun OSS storage backend.
pub struct OssStorage {
    pub(super) http: Client,
    pub(super) config: OssConfig,
    pub(super) base: String,
    files: Mutex<HashMap<String, FileEntry>>,
    exists_cache: Mutex<HashMap<String, Instant>>,
}

impl OssStorage {
    /// Build a backend from the `CLUSTER_STORAGE_OPTIONS` JSON object.
    pub fn new(opts: &Value) -> Result<Self> {
        let config = OssConfig::parse(opts)?;
        let base = build_base_url(&config)?;
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(300))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(16)
            .tcp_nodelay(true)
            .build()?;
        Ok(OssStorage {
            http,
            config,
            base,
            files: Mutex::new(HashMap::new()),
            exists_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Full object key for a storage-relative path.
    fn object_key(&self, path: &str) -> String {
        join_object_key(&self.config.prefix, path)
    }

    /// Public URL of an object.
    pub(super) fn object_url(&self, key: &str) -> String {
        format!("{}/{}", self.base, encode_path(key))
    }
}

#[async_trait]
impl Storage for OssStorage {
    async fn check(&self) -> Result<bool> {
        let key = self.object_key(".check");
        let body = Bytes::from(now_ms().to_string());
        let outcome = self.put_object(&key, body).await;
        if let Err(err) = self.delete_object(&key).await {
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
        self.put_object(&key, Bytes::copy_from_slice(content))
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
        if self.head_object(&key).await? {
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
            for object in self.list_all().await? {
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
                            path: strip_prefix_key(&object.key, &self.config.prefix).to_string(),
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
        for object in self.list_all().await? {
            // The reserved probe folder is not part of the master's list.
            if crate::storage::measure::is_reserved(strip_prefix_key(
                &object.key,
                &self.config.prefix,
            )) {
                continue;
            }
            let hash = basename(&object.key);
            if wanted.contains(hash) {
                continue;
            }
            info!(path = %object.key, "delete expire file");
            self.delete_object(&object.key).await?;
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
        let bytes = get_size(known.map(|entry| entry.size).unwrap_or(0), req.range).max(0) as u64;

        if self.config.proxy {
            let date = http_date();
            let auth = self.authorization("GET", &key, &[], &date);
            let mut request = self
                .http
                .get(self.object_url(&key))
                .header(header::DATE, date.as_str())
                .header(header::AUTHORIZATION, auth.as_str());
            if let Some(range) = req.range {
                request = request.header(header::RANGE, range);
            }
            let upstream = request.send().await?;
            let builder =
                copy_passthrough(&upstream, Response::builder().status(upstream.status()));
            let body = Body::from_stream(
                upstream
                    .bytes_stream()
                    .map(|result| result.map_err(std::io::Error::other)),
            );
            let response = builder
                .body(body)
                .map_err(|err| Error::other(format!("failed to build proxy response: {err}")))?;
            return Ok((response, ServeStat { bytes, hits: 1 }));
        }

        let expires = Utc::now().timestamp() + 60;
        let mut subresources: Vec<(String, String)> = Vec::new();
        if let Some(disposition) = &disposition {
            subresources.push((
                "response-content-disposition".to_string(),
                disposition.clone(),
            ));
        }
        let resource = canonicalized_resource(&self.config.bucket, &key, &subresources);
        let signature = hmac_sha1_base64(
            &self.config.access_key_secret,
            &string_to_sign_presign(expires, &resource),
        );
        let mut query: Vec<(String, String)> = vec![
            (
                "OSSAccessKeyId".to_string(),
                self.config.access_key_id.clone(),
            ),
            ("Expires".to_string(), expires.to_string()),
            ("Signature".to_string(), signature),
        ];
        if let Some(disposition) = disposition {
            query.push(("response-content-disposition".to_string(), disposition));
        }
        let query_string = query
            .iter()
            .map(|(name, value)| format!("{}={}", encode_component(name), encode_component(value)))
            .collect::<Vec<_>>()
            .join("&");
        let location = format!("{}?{}", self.object_url(&key), query_string);
        let response = Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, location.as_str())
            .body(Body::empty())
            .map_err(|err| Error::other(format!("failed to build redirect response: {err}")))?;
        Ok((response, ServeStat { bytes, hits: 1 }))
    }
}
