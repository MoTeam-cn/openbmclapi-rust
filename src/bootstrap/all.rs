//! Orchestration: one process, one storage, one verification pass.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{error, info};

use crate::cluster::Cluster;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::storage;

use super::instance::{self, Role};

/// Run one agent process to completion.
pub async fn run(config: Config) -> Result<()> {
    let configs = config.split()?;
    info!("booting openbmclapi {}", crate::VERSION);
    crate::server::install_crypto_provider();

    // One backend for every instance: that is what turns the file verification
    // into a single pass instead of one per node.
    let backend = storage::create(&configs[0])?;
    if configs.len() > 1 {
        info!(
            instances = configs.len(),
            "several nodes in one process, sharing the storage backend"
        );
    }

    let (gate, _) = watch::channel(None);
    let mut tasks = Vec::with_capacity(configs.len());
    for (index, config) in configs.into_iter().enumerate() {
        let cluster = Cluster::with_storage(config, Arc::clone(&backend))?;
        let role = if index == 0 {
            Role::leader(gate.clone())
        } else {
            Role::follower(gate.subscribe())
        };
        tasks.push(tokio::spawn(instance::run(cluster, role)));
    }

    let mut failure = None;
    for task in tasks {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                error!(error = %e, "an instance stopped");
                failure.get_or_insert(e);
            }
            Err(e) => {
                error!(error = %e, "an instance stopped unexpectedly");
                failure.get_or_insert_with(|| Error::Other(format!("instance task failed: {e}")));
            }
        }
    }
    match failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
