//! Several node identities in one process.
//!
//! A node is a cluster id, a secret and a port; the storage backend, the sync
//! tuning and the logging are shared. Running several of them in one process is
//! what lets a machine serve under more than one id while downloading the file
//! set only once.

use std::collections::HashSet;

use crate::error::{Error, Result};

use super::env::Config;

/// One node identity, as written under `instances` or in `CLUSTER_INSTANCES`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Instance {
    pub cluster_id: String,
    pub cluster_secret: String,
    pub port: u16,
    /// Port advertised to the master; defaults to `port`.
    pub cluster_public_port: Option<u16>,
    /// Address advertised to the master; probed when unset.
    pub cluster_ip: Option<String>,
}

impl Config {
    /// Check the identity fields, whether one node or several were configured.
    pub fn validate(&self) -> Result<()> {
        if self.instances.is_empty() {
            if self.cluster_id.is_empty() {
                return Err(Error::Config("cluster_id is required".into()));
            }
            if self.cluster_secret.is_empty() {
                return Err(Error::Config("cluster_secret is required".into()));
            }
            return Ok(());
        }
        if !self.cluster_id.is_empty() || !self.cluster_secret.is_empty() {
            return Err(Error::Config(
                "cluster_id / cluster_secret and instances are mutually exclusive".into(),
            ));
        }
        let mut ports = HashSet::new();
        for (index, instance) in self.instances.iter().enumerate() {
            let label = format!("instances[{index}]");
            if instance.cluster_id.is_empty() {
                return Err(Error::Config(format!("{label}: cluster_id is required")));
            }
            if instance.cluster_secret.is_empty() {
                return Err(Error::Config(format!(
                    "{label}: cluster_secret is required"
                )));
            }
            if !ports.insert(instance.port) {
                return Err(Error::Config(format!(
                    "{label}: port {} is already used by another instance",
                    instance.port
                )));
            }
        }
        Ok(())
    }

    /// Expand into one configuration per node identity.
    ///
    /// A single-node configuration expands to itself, so nothing downstream has
    /// to know which form the operator used.
    pub fn split(&self) -> Result<Vec<Config>> {
        self.validate()?;
        if self.instances.is_empty() {
            return Ok(vec![self.clone()]);
        }
        // UPnP maps one public port, so it cannot front several nodes.
        if self.instances.len() > 1 && self.enable_upnp {
            return Err(Error::Config(
                "enable_upnp cannot map several instances; forward their ports yourself".into(),
            ));
        }
        let mut configs = Vec::with_capacity(self.instances.len());
        for instance in &self.instances {
            let mut config = self.clone();
            config.cluster_id = instance.cluster_id.clone();
            config.cluster_secret = instance.cluster_secret.clone();
            config.port = instance.port;
            config.cluster_public_port = instance.cluster_public_port.unwrap_or(instance.port);
            config.cluster_ip = instance.cluster_ip.clone();
            config.instances = Vec::new();
            configs.push(config);
        }
        Ok(configs)
    }
}

#[cfg(test)]
#[path = "instances_test.rs"]
mod tests;
