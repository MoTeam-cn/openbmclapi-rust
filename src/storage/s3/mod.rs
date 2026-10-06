//! MinIO / S3-compatible object storage.
//!
//! Port of `src/storage/minio.storage.ts`. The port cannot use the MinIO
//! SDK, so requests are signed with AWS Signature Version 4 and objects are
//! addressed path-style (`<scheme>://<authority>/<bucket>/<key>`).

mod client;
mod endpoint;
mod sigv4;
mod storage;
mod xml;

pub use storage::MinioStorage;
