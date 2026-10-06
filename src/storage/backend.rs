//! The storage contract every backend implements.

use async_trait::async_trait;
use axum::response::Response;

use crate::error::Result;
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

    /// Whether bandwidth probes may be served from this backend's own link.
    ///
    /// A backend that answers false never hands out a stored probe; the agent
    /// generates the payload instead. Pools aggregate this over their members.
    fn measure_redirect(&self) -> bool {
        false
    }

    /// Serve the stored bandwidth probe for `size_mib`, if this backend holds one.
    ///
    /// None means the caller should generate the payload in process.
    async fn serve_measure(&self, size_mib: u64) -> Result<Option<(Response, ServeStat)>> {
        let _ = size_mib;
        Ok(None)
    }
}
