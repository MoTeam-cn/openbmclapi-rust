//! YAML configuration file loading, layered over the environment values.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{Error, Result};

use super::env::{Config, StorageSource};
use super::instances::Instance;
use super::value::{boolean, optional_string, port, positive, size_list, string, type_error};
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
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let document = yaml::parse(&text)
                .map_err(|e| Error::Config(format!("{}：{e}", path.display())))?;
            let root = document
                .as_object()
                .ok_or_else(|| Error::Config(format!("{}：文档根必须是映射", path.display())))?;
            apply(&mut config, root)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(Error::Config(format!("无法读取 {}：{e}", path.display())));
        }
    }
    config.validate()?;
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
            "enable_upnp" => config.enable_upnp = boolean(key, value)?,
            "ssl_key" => config.ssl_key = optional_string(key, value)?,
            "ssl_cert" => config.ssl_cert = optional_string(key, value)?,
            "bmclapi_base" => config.bmclapi_base = string(key, value)?,
            "no_daemon" => config.no_daemon = boolean(key, value)?,
            "no_fast_enable" => config.no_fast_enable = boolean(key, value)?,
            "log_level" => config.log_level = string(key, value)?,
            "plain_log" => config.plain_log = boolean(key, value)?,
            "sync_memory_budget" => config.sync_memory_budget = positive(key, value)?,
            "measure_sizes" => config.measure_sizes = size_list(key, value)?,
            "log_dir" => {
                config.log_dir = optional_string(key, value)?.map(std::path::PathBuf::from)
            }
            "log_format" => config.log_format = string(key, value)?,
            "measure_redirect" => config.measure_redirect = boolean(key, value)?,
            "instances" => config.instances = instance_list(value)?,
            "storage" => apply_storage(config, value)?,
            other => {
                return Err(Error::Config(format!("未知的配置项 {other:?}")));
            }
        }
    }
    Ok(())
}

/// Read the list of node identities.
fn instance_list(value: &Value) -> Result<Vec<Instance>> {
    let items = value
        .as_array()
        .ok_or_else(|| type_error("instances", "a list", value))?;
    if items.is_empty() {
        return Err(Error::Config("instances 至少要有一项".into()));
    }
    let mut parsed = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        parsed.push(parse_instance(&format!("instances[{index}]"), item)?);
    }
    Ok(parsed)
}

/// Read one node identity entry.
fn parse_instance(label: &str, value: &Value) -> Result<Instance> {
    let map = value
        .as_object()
        .ok_or_else(|| type_error(label, "a mapping", value))?;
    for key in map.keys() {
        if !matches!(
            key.as_str(),
            "cluster_id" | "cluster_secret" | "port" | "cluster_public_port" | "cluster_ip"
        ) {
            return Err(Error::Config(format!("{label}：未知的键 {key:?}")));
        }
    }
    let text = |name: &str| match map.get(name) {
        Some(value) => string(&format!("{label}.{name}"), value),
        None => Ok(String::new()),
    };
    Ok(Instance {
        cluster_id: text("cluster_id")?,
        cluster_secret: text("cluster_secret")?,
        port: match map.get("port") {
            Some(value) => port(&format!("{label}.port"), value)?,
            None => 0,
        },
        cluster_public_port: match map.get("cluster_public_port") {
            Some(value) => Some(port(&format!("{label}.cluster_public_port"), value)?),
            None => None,
        },
        cluster_ip: match map.get("cluster_ip") {
            Some(value) => optional_string(&format!("{label}.cluster_ip"), value)?,
            None => None,
        },
    })
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
            "storage：'type'/'options' 与 'sources' 互斥".into(),
        ));
    }
    let parsed = if let Some(sources) = sources {
        let list = sources
            .as_array()
            .ok_or_else(|| type_error("storage.sources", "a list", sources))?;
        if list.is_empty() {
            return Err(Error::Config("storage.sources 至少要有一个源".into()));
        }
        let mut parsed = Vec::with_capacity(list.len());
        for (index, item) in list.iter().enumerate() {
            let source = parse_source(&format!("storage.sources[{index}]"), item)?;
            if source.kind == "file" {
                return Err(Error::Config(format!(
                    "storage.sources[{index}]：本地 file 后端不能加入多源池"
                )));
            }
            parsed.push(source);
        }
        parsed
    } else if single {
        vec![parse_source("storage", value)?]
    } else {
        return Err(Error::Config("storage 必须设置 'type' 或 'sources'".into()));
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
        if key != "type" && key != "options" && key != "measure_redirect" {
            return Err(Error::Config(format!("{label}：未知的键 {key:?}")));
        }
    }
    let kind = map
        .get("type")
        .ok_or_else(|| Error::Config(format!("{label}：必须提供 'type'")))?;
    let kind = kind
        .as_str()
        .ok_or_else(|| type_error(&format!("{label}.type"), "a string", kind))?;
    if kind.is_empty() {
        return Err(Error::Config(format!("{label}：'type' 不能为空")));
    }
    let options = match map.get("options") {
        None | Some(Value::Null) => Value::Null,
        Some(options @ Value::Object(_)) => options.clone(),
        Some(other) => return Err(type_error(&format!("{label}.options"), "a mapping", other)),
    };
    let measure_redirect = match map.get("measure_redirect") {
        Some(value) => Some(boolean(&format!("{label}.measure_redirect"), value)?),
        None => None,
    };
    Ok(StorageSource {
        kind: kind.to_string(),
        options,
        measure_redirect,
    })
}

#[cfg(test)]
#[path = "file_test.rs"]
mod tests;
