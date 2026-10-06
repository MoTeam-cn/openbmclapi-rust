//! Backend selection for the configured storage sources.
//!
//! One source is used directly; several are wrapped in a
//! [`MultiStorage`](super::multi::MultiStorage) pool that spreads reads and
//! replicates writes. The configuration layer rejects `file` inside a pool.

use std::sync::Arc;

use crate::config::{Config, StorageSource};
use crate::error::{Error, Result};

use super::backend::Storage;
use super::multi::MultiStorage;
use super::{alist, file, oss, s3, webdav};

/// Build the storage backend(s) named by the configuration.
pub fn create(config: &Config) -> Result<Arc<dyn Storage>> {
    let sources = config.storage_sources.as_slice();
    match sources {
        [] => Err(Error::Config("no storage source configured".into())),
        [single] => build(config, single),
        many => {
            let mut built = Vec::with_capacity(many.len());
            for source in many {
                built.push(build(config, source)?);
            }
            Ok(Arc::new(MultiStorage::new(built)?))
        }
    }
}

/// Build one backend. `file` takes no options and resolves to the local cache.
fn build(config: &Config, source: &StorageSource) -> Result<Arc<dyn Storage>> {
    let storage: Arc<dyn Storage> = match source.kind.as_str() {
        "file" => Arc::new(file::FileStorage::new(config.cache_dir())),
        "minio" => Arc::new(s3::MinioStorage::new(&source.options)?),
        "oss" => Arc::new(oss::OssStorage::new(&source.options)?),
        "webdav" => Arc::new(webdav::WebdavStorage::new(&source.options)?),
        "alist" => Arc::new(alist::AlistStorage::new(&source.options)?),
        other => return Err(Error::Config(format!("unknown storage type: {other}"))),
    };
    Ok(storage)
}
