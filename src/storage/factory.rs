//! Backend selection for the configured storage sources.
//!
//! Every configuration ends up as a [`MultiStorage`](super::multi::MultiStorage)
//! pool, which spreads reads and replicates writes. A single source is a pool of
//! one, because the pool is also where the bandwidth-probe policy lives. The
//! configuration layer rejects `file` alongside anything else.

use std::sync::Arc;

use crate::config::{Config, StorageSource};
use crate::error::{Error, Result};

use super::backend::Storage;
use super::multi::MultiStorage;
use super::{alist, file, oss, s3, webdav};

/// Build the storage backend(s) named by the configuration.
pub fn create(config: &Config) -> Result<Arc<dyn Storage>> {
    let sources = config.storage_sources.as_slice();
    if sources.is_empty() {
        return Err(Error::Config("没有配置任何存储源".into()));
    }
    let mut built = Vec::with_capacity(sources.len());
    let mut policy = Vec::with_capacity(sources.len());
    for source in sources {
        built.push(build(config, source)?);
        // The local cache is never seeded with probes, so it can never serve one.
        policy.push(source.kind != "file" && source.measure_redirect.unwrap_or(true));
    }
    Ok(Arc::new(MultiStorage::with_measure_redirect(
        built, policy,
    )?))
}

/// Build one backend. `file` takes no options and resolves to the local cache.
fn build(config: &Config, source: &StorageSource) -> Result<Arc<dyn Storage>> {
    let storage: Arc<dyn Storage> = match source.kind.as_str() {
        "file" => Arc::new(file::FileStorage::new(config.cache_dir())),
        "minio" => Arc::new(s3::MinioStorage::new(&source.options)?),
        "oss" => Arc::new(oss::OssStorage::new(&source.options)?),
        "webdav" => Arc::new(webdav::WebdavStorage::new(&source.options)?),
        "alist" => Arc::new(alist::AlistStorage::new(&source.options)?),
        other => return Err(Error::Config(format!("未知的存储类型：{other}"))),
    };
    Ok(storage)
}
