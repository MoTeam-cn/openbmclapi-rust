//! Backend selection for `CLUSTER_STORAGE`.

use std::sync::Arc;

use crate::config::Config;
use crate::error::{Error, Result};

use super::backend::Storage;
use super::{alist, file, oss, s3, webdav};

/// Instantiate the backend named by `CLUSTER_STORAGE`.
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
        other => return Err(Error::Config(format!("unknown storage type: {other}"))),
    };
    Ok(storage)
}
