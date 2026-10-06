//! Configuration from the process environment.
//!
//! Every field maps onto a CLUSTER_*-style variable of the Node
//! implementation. This is the baseline layer; file overrides it.

use std::env;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::util::parse_bool;

use super::instances::Instance;

/// Default master endpoint.
pub const DEFAULT_BMCLAPI_BASE: &str = "https://openbmclapi.bangbang93.com";
/// Default listening port.
pub const DEFAULT_PORT: u16 = 4000;
/// Default cap on the download bytes buffered during one sync pass, in MiB.
pub const DEFAULT_SYNC_MEMORY_MIB: u64 = 256;
/// Sizes, in MiB, of the speed-test objects seeded into the storage backend.
pub const DEFAULT_MEASURE_SIZES: &[u64] = &[0, 1, 2, 4, 8, 16, 32, 64, 128];
/// Largest probe the measure route accepts, in MiB.
pub const MAX_MEASURE_MIB: u64 = 200;

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
    /// MiB of download bodies that may be buffered at once during a sync.
    pub sync_memory_budget: u64,
    /// MiB sizes of the probes seeded into the storage backend; empty disables.
    pub measure_sizes: Vec<u64>,
    /// Directory for per-category log files; unset keeps logging on the console.
    pub log_dir: Option<std::path::PathBuf>,
    /// Shape of the log stream: `pretty` or `json`.
    pub log_format: String,
    /// Node identities when several run in this process; empty means one node,
    /// described by the identity fields above.
    pub instances: Vec<Instance>,
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

/// Parse a comma-separated list of MiB sizes.
///
/// An empty string disables the stored probes. A zero, out-of-range or
/// unparsable entry is an error: silently dropping it would leave a size that
/// the route advertises as available permanently unserved.
pub(super) fn parse_size_list(raw: &str) -> Result<Vec<u64>> {
    let mut sizes = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let size: u64 = part
            .parse()
            .map_err(|e| Error::Config(format!("invalid speed-test size {part:?}: {e}")))?;
        if size == 0 || size > MAX_MEASURE_MIB {
            return Err(Error::Config(format!(
                "speed-test size {size} is outside 1..={MAX_MEASURE_MIB}"
            )));
        }
        if !sizes.contains(&size) {
            sizes.push(size);
        }
    }
    Ok(sizes)
}

fn num_var(name: &str) -> Option<u64> {
    var(name).and_then(|v| v.trim().parse::<u64>().ok())
}

/// Read `CLUSTER_INSTANCES`, a JSON array of node identities.
fn parse_instances() -> Result<Vec<Instance>> {
    let Some(raw) = var("CLUSTER_INSTANCES") else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&raw).map_err(|e| Error::Config(format!("invalid CLUSTER_INSTANCES: {e}")))
}

impl Config {
    /// Read the configuration from the process environment.
    pub fn from_env() -> Result<Self> {
        let instances = parse_instances()?;
        if !instances.is_empty() {
            for key in [
                "CLUSTER_ID",
                "CLUSTER_SECRET",
                "CLUSTER_PORT",
                "CLUSTER_PUBLIC_PORT",
                "CLUSTER_IP",
            ] {
                if var(key).is_some() {
                    return Err(Error::Config(format!(
                        "{key} cannot be combined with CLUSTER_INSTANCES; each instance carries its own"
                    )));
                }
            }
        }
        // An identity is only required when no instance list supplies one, so
        // a file that lists instances works with an otherwise empty
        // environment. validate() is what enforces it.
        let (cluster_id, cluster_secret) = if instances.is_empty() {
            (
                var("CLUSTER_ID").unwrap_or_default(),
                var("CLUSTER_SECRET").unwrap_or_default(),
            )
        } else {
            (String::new(), String::new())
        };
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
            no_fast_enable: bool_var("NO_FAST_ENABLE"),
            log_dir: var("LOG_DIR").map(std::path::PathBuf::from),
            log_format: var("LOG_FORMAT").unwrap_or_else(|| "pretty".to_string()),
            instances,
            log_level: var("LOGLEVEL").unwrap_or_else(|| "info".to_string()),
            plain_log: bool_var("PLAIN_LOG"),
            sync_memory_budget: num_var("SYNC_MEMORY_BUDGET").unwrap_or(DEFAULT_SYNC_MEMORY_MIB),
            // Read this one directly: `var` treats an empty value as unset,
            // and an explicitly empty list is how seeding is turned off.
            measure_sizes: match env::var("MEASURE_SIZES") {
                Ok(raw) => parse_size_list(&raw)?,
                Err(_) => DEFAULT_MEASURE_SIZES.to_vec(),
            },
            flavor,
        })
    }

    /// Whether every configured backend is the local disk cache.
    ///
    /// The local backend is never seeded with measure objects: it is the same
    /// disk the agent runs on, so generating a payload on the fly is cheaper
    /// than storing a second copy of it.
    pub fn uses_local_storage(&self) -> bool {
        self.storage_sources
            .iter()
            .all(|source| source.kind == "file")
    }

    /// Directory used for the local file cache.
    pub fn cache_dir(&self) -> std::path::PathBuf {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("cache")
    }

    /// Temporary working directory for certificates, mirroring the Node agent.
    ///
    /// Keyed by identity: several instances in one process would otherwise
    /// overwrite each other's certificate.
    pub fn tmp_dir(&self) -> std::path::PathBuf {
        let id: String = self
            .cluster_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let id = if id.is_empty() { "node" } else { &id };
        std::env::temp_dir().join("openbmclapi").join(id)
    }
}

/// Serialises tests that mutate the process environment, which is global.
#[cfg(test)]
pub(super) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[path = "env_test.rs"]
mod tests;
