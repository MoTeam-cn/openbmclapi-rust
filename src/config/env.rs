//! Configuration from the process environment.
//!
//! Every field maps onto a CLUSTER_*-style variable of the Node
//! implementation. This is the baseline layer; file overrides it.

use std::env;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::util::parse_bool;

/// Default master endpoint.
pub const DEFAULT_BMCLAPI_BASE: &str = "https://openbmclapi.bangbang93.com";
/// Default listening port.
pub const DEFAULT_PORT: u16 = 4000;

/// Runtime description advertised to the master.
#[derive(Debug, Clone, Serialize)]
pub struct Flavor {
    pub runtime: String,
    pub storage: String,
    /// Marks this implementation so the master can tell the port from the Node agent.
    pub implementation: String,
    /// Where this port lives, for operators reading the master's node list.
    pub repo: String,
}

/// One backend in the storage configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct StorageSource {
    /// Backend name, e.g. "file", "alist" or "webdav".
    pub kind: String,
    /// Backend-specific options; null when the backend takes none.
    pub options: serde_json::Value,
}

/// Agent configuration, read from the process environment.
///
/// Field names mirror the CLUSTER_* environment variables of the Node
/// implementation.
#[derive(Debug, Clone)]
pub struct Config {
    pub cluster_id: String,
    pub cluster_secret: String,
    pub cluster_ip: Option<String>,
    pub port: u16,
    pub cluster_public_port: u16,
    pub byoc: bool,
    pub disable_access_log: bool,
    pub disable_sign: bool,
    pub enable_nginx: bool,
    pub enable_upnp: bool,
    pub storage: String,
    pub storage_opts: Option<serde_json::Value>,
    /// Every configured backend, in order; always at least one entry.
    pub storage_sources: Vec<StorageSource>,
    pub ssl_key: Option<String>,
    pub ssl_cert: Option<String>,
    pub bmclapi_base: String,
    pub no_daemon: bool,
    pub no_fast_enable: bool,
    pub log_level: String,
    pub plain_log: bool,
    pub flavor: Flavor,
}

fn var(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

fn bool_var(name: &str) -> bool {
    var(name).map(|v| parse_bool(&v)).unwrap_or(false)
}

fn required(name: &str) -> Result<String> {
    var(name).ok_or_else(|| Error::Config(format!("missing required environment variable {name}")))
}

impl Config {
    /// Read the configuration from the process environment.
    pub fn from_env() -> Result<Self> {
        let cluster_id = required("CLUSTER_ID")?;
        let cluster_secret = required("CLUSTER_SECRET")?;
        let port = match var("CLUSTER_PORT") {
            Some(raw) => raw
                .parse::<u16>()
                .map_err(|e| Error::Config(format!("invalid CLUSTER_PORT {raw:?}: {e}")))?,
            None => DEFAULT_PORT,
        };
        let cluster_public_port = match var("CLUSTER_PUBLIC_PORT") {
            Some(raw) => raw
                .parse::<u16>()
                .map_err(|e| Error::Config(format!("invalid CLUSTER_PUBLIC_PORT {raw:?}: {e}")))?,
            None => port,
        };
        let storage = var("CLUSTER_STORAGE").unwrap_or_else(|| "file".to_string());
        let storage_opts = match var("CLUSTER_STORAGE_OPTIONS") {
            Some(raw) => Some(
                serde_json::from_str::<serde_json::Value>(&raw)
                    .map_err(|e| Error::Config(format!("invalid CLUSTER_STORAGE_OPTIONS: {e}")))?,
            ),
            None => None,
        };
        let storage_sources = vec![StorageSource {
            kind: storage.clone(),
            options: storage_opts.clone().unwrap_or(serde_json::Value::Null),
        }];
        let flavor = Flavor {
            runtime: format!("Rust/{}", crate::VERSION),
            storage: storage.clone(),
            implementation: "openbmclapi-rust".to_string(),
            repo: "https://github.com/MoTeam-cn/openbmclapi-rust".to_string(),
        };
        Ok(Config {
            cluster_id,
            cluster_secret,
            cluster_ip: var("CLUSTER_IP"),
            port,
            cluster_public_port,
            byoc: bool_var("CLUSTER_BYOC"),
            disable_access_log: bool_var("DISABLE_ACCESS_LOG"),
            disable_sign: bool_var("DISABLE_SIGN"),
            enable_nginx: bool_var("ENABLE_NGINX"),
            enable_upnp: bool_var("ENABLE_UPNP"),
            storage,
            storage_opts,
            storage_sources,
            ssl_key: var("SSL_KEY"),
            ssl_cert: var("SSL_CERT"),
            bmclapi_base: var("CLUSTER_BMCLAPI")
                .unwrap_or_else(|| DEFAULT_BMCLAPI_BASE.to_string()),
            no_daemon: bool_var("NO_DAEMON"),
            no_fast_enable: var("NO_FAST_ENABLE").map(|v| v == "true").unwrap_or(false),
            log_level: var("LOGLEVEL").unwrap_or_else(|| "info".to_string()),
            plain_log: bool_var("PLAIN_LOG"),
            flavor,
        })
    }

    /// Directory used for the local file cache.
    pub fn cache_dir(&self) -> std::path::PathBuf {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("cache")
    }

    /// Temporary working directory for certificates, mirroring the Node agent.
    pub fn tmp_dir(&self) -> std::path::PathBuf {
        std::env::temp_dir().join("openbmclapi")
    }
}

/// Serialises tests that mutate the process environment, which is global.
#[cfg(test)]
pub(super) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[path = "env_test.rs"]
mod tests;
