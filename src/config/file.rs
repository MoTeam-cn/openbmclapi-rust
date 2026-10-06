//! YAML configuration file loading, layered over the environment values.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::util::parse_bool;

use super::env::{Config, StorageSource};
use super::yaml;

/// File name used when no path is given, relative to the working directory.
pub const DEFAULT_FILE: &str = "config.yaml";

/// Load the configuration from a path, defaulting to config.yaml.
///
/// The environment is read first and every key present in the file overrides
/// it, so a file alone can configure the agent. A missing file leaves the
/// environment values untouched.
pub fn load(path: Option<&Path>) -> Result<Config> {
    load_file(path)
}

impl Config {
    /// Load from a file (default config.yaml) layered over the environment.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        load_file(path)
    }
}

fn load_file(path: Option<&Path>) -> Result<Config> {
    let path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_FILE));
    let mut config = Config::from_env()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(config),
        Err(e) => {
            return Err(Error::Config(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    let document =
        yaml::parse(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
    let root = document.as_object().ok_or_else(|| {
        Error::Config(format!(
            "{}: the document root must be a mapping",
            path.display()
        ))
    })?;
    apply(&mut config, root)?;
    Ok(config)
}

/// Override config with every key present in root.
fn apply(config: &mut Config, root: &Map<String, Value>) -> Result<()> {
    for (key, value) in root {
        match key.as_str() {
            "cluster_id" => config.cluster_id = string(key, value)?,
            "cluster_secret" => config.cluster_secret = string(key, value)?,
            "cluster_ip" => config.cluster_ip = optional_string(key, value)?,
            "port" => config.port = port(key, value)?,
            "cluster_public_port" => config.cluster_public_port = port(key, value)?,
            "byoc" => config.byoc = boolean(key, value)?,
            "disable_access_log" => config.disable_access_log = boolean(key, value)?,
            "disable_sign" => config.disable_sign = boolean(key, value)?,
            "enable_nginx" => config.enable_nginx = boolean(key, value)?,
            "enable_upnp" => config.enable_upnp = boolean(key, value)?,
            "ssl_key" => config.ssl_key = optional_string(key, value)?,
            "ssl_cert" => config.ssl_cert = optional_string(key, value)?,
            "bmclapi_base" => config.bmclapi_base = string(key, value)?,
            "no_daemon" => config.no_daemon = boolean(key, value)?,
            "no_fast_enable" => config.no_fast_enable = boolean(key, value)?,
            "log_level" => config.log_level = string(key, value)?,
            "plain_log" => config.plain_log = boolean(key, value)?,
            "sync_memory_budget" => config.sync_memory_budget = positive(key, value)?,
            "speedtest_sizes" => config.speedtest_sizes = size_list(key, value)?,
            "storage" => apply_storage(config, value)?,
            other => {
                return Err(Error::Config(format!(
                    "unknown configuration key {other:?}"
                )));
            }
        }
    }
    Ok(())
}

/// Replace the storage configuration with the one described by value.
fn apply_storage(config: &mut Config, value: &Value) -> Result<()> {
    let map = value
        .as_object()
        .ok_or_else(|| type_error("storage", "a mapping", value))?;
    let single = map.contains_key("type") || map.contains_key("options");
    let sources = map.get("sources");
    if single && sources.is_some() {
        return Err(Error::Config(
            "storage: 'type'/'options' and 'sources' are mutually exclusive".into(),
        ));
    }
    let parsed = if let Some(sources) = sources {
        let list = sources
            .as_array()
            .ok_or_else(|| type_error("storage.sources", "a list", sources))?;
        if list.is_empty() {
            return Err(Error::Config(
                "storage.sources must contain at least one source".into(),
            ));
        }
        let mut parsed = Vec::with_capacity(list.len());
        for (index, item) in list.iter().enumerate() {
            let source = parse_source(&format!("storage.sources[{index}]"), item)?;
            if source.kind == "file" {
                return Err(Error::Config(format!(
                    "storage.sources[{index}]: the local file backend cannot join a multi-source pool"
                )));
            }
            parsed.push(source);
        }
        parsed
    } else if single {
        vec![parse_source("storage", value)?]
    } else {
        return Err(Error::Config(
            "storage must set either 'type' or 'sources'".into(),
        ));
    };
    let first = &parsed[0];
    config.storage = first.kind.clone();
    config.storage_opts = match &first.options {
        Value::Null => None,
        options => Some(options.clone()),
    };
    config.flavor.storage = config.storage.clone();
    config.storage_sources = parsed;
    Ok(())
}

/// Read one {type, options} source entry.
fn parse_source(label: &str, value: &Value) -> Result<StorageSource> {
    let map = value
        .as_object()
        .ok_or_else(|| type_error(label, "a mapping", value))?;
    for key in map.keys() {
        if key != "type" && key != "options" {
            return Err(Error::Config(format!("{label}: unknown key {key:?}")));
        }
    }
    let kind = map
        .get("type")
        .ok_or_else(|| Error::Config(format!("{label}: 'type' is required")))?;
    let kind = kind
        .as_str()
        .ok_or_else(|| type_error(&format!("{label}.type"), "a string", kind))?;
    if kind.is_empty() {
        return Err(Error::Config(format!("{label}: 'type' must not be empty")));
    }
    let options = match map.get("options") {
        None | Some(Value::Null) => Value::Null,
        Some(options @ Value::Object(_)) => options.clone(),
        Some(other) => return Err(type_error(&format!("{label}.options"), "a mapping", other)),
    };
    Ok(StorageSource {
        kind: kind.to_string(),
        options,
    })
}

fn string(key: &str, value: &Value) -> Result<String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        other => Err(type_error(key, "a string", other)),
    }
}

fn optional_string(key: &str, value: &Value) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        other => string(key, other).map(Some),
    }
}

fn boolean(key: &str, value: &Value) -> Result<bool> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        Value::String(text) => Ok(parse_bool(text)),
        Value::Number(number) if number.as_u64() == Some(0) => Ok(false),
        Value::Number(number) if number.as_u64() == Some(1) => Ok(true),
        other => Err(type_error(key, "a boolean", other)),
    }
}

/// MiB sizes, written either as a sequence or as a comma-separated string.
fn size_list(key: &str, value: &Value) -> Result<Vec<u64>> {
    let raw = match value {
        Value::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::Number(number) => parts.push(number.to_string()),
                    Value::String(text) => parts.push(text.clone()),
                    other => return Err(type_error(key, "a list of sizes", other)),
                }
            }
            parts.join(",")
        }
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => return Err(type_error(key, "a list of sizes", other)),
    };
    super::env::parse_size_list(&raw)
}

fn positive(key: &str, value: &Value) -> Result<u64> {
    let number = match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| type_error(key, "a positive number", value))?,
        Value::String(text) => text
            .trim()
            .parse::<u64>()
            .map_err(|e| Error::Config(format!("{key:?} is not a number: {e}")))?,
        other => return Err(type_error(key, "a positive number", other)),
    };
    if number == 0 {
        return Err(Error::Config(format!("{key:?} must be greater than zero")));
    }
    Ok(number)
}

fn port(key: &str, value: &Value) -> Result<u16> {
    let number = match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| type_error(key, "a port number", value))?,
        Value::String(text) => text
            .trim()
            .parse::<u64>()
            .map_err(|e| Error::Config(format!("{key:?} is not a valid port: {e}")))?,
        other => return Err(type_error(key, "a port number", other)),
    };
    u16::try_from(number)
        .map_err(|_| Error::Config(format!("{key:?} port {number} is out of range")))
}

fn type_error(key: &str, expected: &str, value: &Value) -> Error {
    Error::Config(format!("{key:?} must be {expected}, found {}", kind(value)))
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "a mapping",
    }
}

#[cfg(test)]
#[path = "file_test.rs"]
mod tests;
