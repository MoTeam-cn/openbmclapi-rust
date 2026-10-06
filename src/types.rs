use serde::{Deserialize, Serialize};

/// A single entry of the master file list.
///
/// Mirrors `IFileInfo` from the Node implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub hash: String,
    pub size: i64,
    pub mtime: i64,
}

impl FileInfo {
    /// The basename of the remote path, used for `content-disposition`.
    pub fn basename(&self) -> &str {
        crate::util::basename(&self.path)
    }
}

/// The full file list returned by `GET openbmclapi/files`.
#[derive(Debug, Clone, Default)]
pub struct FileList {
    pub files: Vec<FileInfo>,
}

/// Result of a garbage collection sweep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcCounter {
    pub count: u64,
    pub size: u64,
}

/// Served-bytes counters reported through `keep-alive`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub hits: u64,
    pub bytes: u64,
}

/// `sync` section of the agent configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct SyncConfig {
    /// Present in the master payload; the agent ignores it.
    #[allow(dead_code)]
    pub source: String,
    pub concurrency: usize,
}

/// `GET openbmclapi/configuration` payload.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfiguration {
    pub sync: SyncConfig,
}

/// Payload sent with `port-check` / `enable`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnableRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub port: u16,
    pub version: String,
    pub byoc: bool,
    pub no_fast_enable: bool,
    pub flavor: crate::config::Flavor,
}

/// Certificate returned by `request-cert`.
#[derive(Debug, Clone, Deserialize)]
pub struct CertPair {
    pub cert: String,
    pub key: String,
}
