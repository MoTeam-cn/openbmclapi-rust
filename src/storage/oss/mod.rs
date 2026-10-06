//! Aliyun OSS object storage.
//!
//! Port of src/storage/oss.storage.ts. Requests are signed with the Aliyun
//! OSS Signature V1 scheme; when proxy is enabled the download is streamed
//! straight through, otherwise a signed redirect URL is returned.

mod client;
mod config;
mod signature;
mod storage;
mod xml;

pub use storage::OssStorage;
