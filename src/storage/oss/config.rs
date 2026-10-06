//! Parsing of `CLUSTER_STORAGE_OPTIONS` and base URL resolution for OSS.

use serde_json::Value;

use crate::error::{Error, Result};

/// Endpoint configuration parsed from `CLUSTER_STORAGE_OPTIONS`.
#[derive(Debug, Clone)]
pub(super) struct OssConfig {
    pub(super) access_key_id: String,
    pub(super) access_key_secret: String,
    pub(super) bucket: String,
    pub(super) internal: bool,
    pub(super) prefix: String,
    pub(super) proxy: bool,
    pub(super) endpoint: Option<String>,
    pub(super) region: Option<String>,
    pub(super) cname: bool,
}

/// Read a non-empty string option.
fn opt_str(opts: &Value, key: &str) -> Option<String> {
    opts.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

/// Read a required non-empty string option.
fn required_str(opts: &Value, key: &str) -> Result<String> {
    opt_str(opts, key).ok_or_else(|| Error::Config(format!("oss storage requires \"{key}\"")))
}

impl OssConfig {
    /// Parse the storage options object, applying the documented defaults.
    pub(super) fn parse(opts: &Value) -> Result<Self> {
        Ok(OssConfig {
            access_key_id: required_str(opts, "accessKeyId")?,
            access_key_secret: required_str(opts, "accessKeySecret")?,
            bucket: required_str(opts, "bucket")?,
            internal: opts
                .get("internal")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            prefix: opt_str(opts, "prefix").unwrap_or_default(),
            proxy: opts.get("proxy").and_then(Value::as_bool).unwrap_or(true),
            endpoint: opt_str(opts, "endpoint"),
            region: opt_str(opts, "region"),
            cname: opts.get("cname").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// Resolve the base URL (scheme + host, no trailing slash) for the bucket.
pub(super) fn build_base_url(config: &OssConfig) -> Result<String> {
    let base = match &config.endpoint {
        Some(endpoint) => {
            let endpoint = endpoint.trim_end_matches('/');
            if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
                endpoint.to_string()
            } else {
                format!("https://{endpoint}")
            }
        }
        None => {
            let mut region_host = match &config.region {
                Some(region) if region.contains(".aliyuncs.com") => region.clone(),
                Some(region) if region.starts_with("oss-") => format!("{region}.aliyuncs.com"),
                Some(region) => format!("oss-{region}.aliyuncs.com"),
                None => "oss-cn-hangzhou.aliyuncs.com".to_string(),
            };
            if config.internal {
                region_host = region_host.replace(".aliyuncs.com", "-internal.aliyuncs.com");
            }
            let host = if config.cname {
                region_host
            } else {
                format!("{}.{}", config.bucket, region_host)
            };
            format!("https://{host}")
        }
    };
    // Caught here rather than on the first request: a malformed endpoint is a
    // configuration mistake, and the supervisor must not loop on it.
    if url::Url::parse(&base).is_err() {
        return Err(Error::Config(format!(
            "oss storage endpoint {base:?} is not a valid URL"
        )));
    }
    Ok(base)
}

#[cfg(test)]
#[path = "config_test.rs"]
mod tests;
