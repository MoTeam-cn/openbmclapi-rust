//! Pluggable file storage backends.
//!
//! Mirrors `src/storage/*.ts` from the Node agent: every backend implements
//! the same six operations and the download route only talks to this trait.

pub mod alist;
pub mod file;
pub mod oss;
pub mod s3;
pub mod webdav;

use std::sync::Arc;

use async_trait::async_trait;
use axum::response::Response;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::types::{FileInfo, GcCounter};

/// Accounting returned by a successful download.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServeStat {
    pub bytes: u64,
    pub hits: u64,
}

/// Everything a backend needs to answer a `/download/:hash` request.
#[derive(Debug, Clone, Copy)]
pub struct ServeRequest<'a> {
    /// Bare content hash as it appears in the URL.
    pub hash: &'a str,
    /// Two-level storage key (`ab/abcdef...`).
    pub hash_path: &'a str,
    /// Raw `Range` request header, if any.
    pub range: Option<&'a str>,
    /// Optional `?name=` override for `content-disposition`.
    pub name: Option<&'a str>,
}

#[async_trait]
pub trait Storage: Send + Sync + 'static {
    /// Optional one-time setup (WebDAV creates its base path here).
    async fn init(&self) -> Result<()> {
        Ok(())
    }

    /// Probe that the backend is writable.
    async fn check(&self) -> Result<bool>;

    /// Persist a downloaded object.
    async fn write_file(&self, path: &str, content: &[u8], file_info: &FileInfo) -> Result<()>;

    /// Whether the object exists in the backend.
    async fn exists(&self, path: &str) -> Result<bool>;

    /// Which of `files` are missing or size-mismatched.
    async fn get_missing_files(&self, files: &[FileInfo]) -> Result<Vec<FileInfo>>;

    /// Delete objects that are no longer part of the master file list.
    async fn gc(&self, files: &[FileInfo]) -> Result<GcCounter>;

    /// Build the HTTP response for a download.
    async fn serve(&self, req: ServeRequest<'_>) -> Result<(Response, ServeStat)>;
}

/// Instantiate the backend selected by `CLUSTER_STORAGE`.
pub fn create(config: &Config) -> Result<Arc<dyn Storage>> {
    let opts = config
        .storage_opts
        .clone()
        .unwrap_or(serde_json::Value::Null);
    let storage: Arc<dyn Storage> = match config.storage.as_str() {
        "file" => Arc::new(file::FileStorage::new(config.cache_dir())),
        "minio" => Arc::new(s3::MinioStorage::new(&opts)?),
        "oss" => Arc::new(oss::OssStorage::new(&opts)?),
        "webdav" => Arc::new(webdav::WebdavStorage::new(&opts)?),
        "alist" => Arc::new(alist::AlistStorage::new(&opts)?),
        other => {
            return Err(Error::Config(format!("unknown storage type: {other}")));
        }
    };
    Ok(storage)
}

/// Resolve a two-level storage key against a local directory.
pub(crate) fn join_key(base: &std::path::Path, key: &str) -> std::path::PathBuf {
    let mut path = base.to_path_buf();
    for segment in key
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
    {
        path.push(segment);
    }
    path
}
