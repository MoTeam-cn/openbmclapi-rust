//! Aliyun OSS object storage.
//!
//! Port of src/storage/oss.storage.ts. Requests are signed with the Aliyun
//! OSS Signature V1 scheme; when proxy is enabled the download is streamed
//! straight through, otherwise a signed redirect URL is returned.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use base64::Engine as _;
use bytes::Bytes;
use chrono::Utc;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::Client;
use serde_json::Value;
use sha1::Sha1;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::error::{Error, Result};
use crate::types::{FileInfo, GcCounter};
use crate::util::{get_size, now_ms};

use super::{ServeRequest, ServeStat, Storage};

/// Characters left unescaped by the OSS URI encoding rule.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// [`UNRESERVED`] plus the path separator, so object keys keep their slashes.
const UNRESERVED_PATH: &AsciiSet = &UNRESERVED.remove(b'/');

/// `encodeURIComponent` semantics, used for content-disposition filenames.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// How long a positive `exists` result stays cached.
const EXISTS_TTL: Duration = Duration::from_secs(3600);

/// The largest page OSS returns from one listing call.
const MAX_KEYS: &str = "1000";

/// One object returned by a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ListedObject {
    key: String,
    size: i64,
}

/// Cached metadata for an object known to be present.
#[derive(Debug, Clone)]
struct FileEntry {
    size: i64,
    path: String,
}

/// Endpoint configuration parsed from `CLUSTER_STORAGE_OPTIONS`.
#[derive(Debug, Clone)]
struct OssConfig {
    access_key_id: String,
    access_key_secret: String,
    bucket: String,
    internal: bool,
    prefix: String,
    proxy: bool,
    endpoint: Option<String>,
    region: Option<String>,
    cname: bool,
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
    fn parse(opts: &Value) -> Result<Self> {
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
fn build_base_url(config: &OssConfig) -> Result<String> {
    if let Some(endpoint) = &config.endpoint {
        let endpoint = endpoint.trim_end_matches('/');
        if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            return Ok(endpoint.to_string());
        }
        return Ok(format!("https://{endpoint}"));
    }
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
    Ok(format!("https://{host}"))
}

/// `path.join(prefix, key)` with `/` separators.
fn join_key(prefix: &str, key: &str) -> String {
    let prefix = prefix.trim_matches('/');
    let key = key.trim_start_matches('/');
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}/{key}")
    }
}

/// Strip a configured prefix from an object key.
fn strip_prefix_key<'a>(key: &'a str, prefix: &str) -> &'a str {
    let prefix = prefix.trim_matches('/');
    if prefix.is_empty() {
        return key;
    }
    key.strip_prefix(prefix)
        .map(|rest| rest.trim_start_matches('/'))
        .unwrap_or(key)
}

/// Last path segment of an object key.
fn basename(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

/// Percent-encode an object key, preserving `/`.
fn encode_path(key: &str) -> String {
    percent_encode(key.as_bytes(), UNRESERVED_PATH).to_string()
}

/// Percent-encode a single query component.
fn encode_component(value: &str) -> String {
    percent_encode(value.as_bytes(), UNRESERVED).to_string()
}

/// Percent-encode a filename the way `encodeURIComponent` does.
fn encode_filename(name: &str) -> String {
    percent_encode(name.as_bytes(), URI_COMPONENT).to_string()
}

/// Base64 HMAC-SHA1 over `data` with `secret`, as OSS V1 requires.
fn hmac_sha1_base64(secret: &str, data: &str) -> String {
    let mut mac =
        Hmac::<Sha1>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(data.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// OSS V1 `CanonicalizedResource`: bucket, key and sorted sub-resources.
fn canonicalized_resource(bucket: &str, key: &str, subresources: &[(String, String)]) -> String {
    let mut resource = format!("/{bucket}/{key}");
    let mut parts: Vec<String> = subresources
        .iter()
        .map(|(name, value)| {
            if value.is_empty() {
                name.clone()
            } else {
                format!("{name}={value}")
            }
        })
        .collect();
    parts.sort();
    if !parts.is_empty() {
        resource.push('?');
        resource.push_str(&parts.join("&"));
    }
    resource
}

/// OSS V1 `StringToSign` for a header-signed request.
///
/// Only used for requests that set neither Content-MD5 nor Content-Type and no
/// `x-oss-*` headers.
fn string_to_sign_v1(method: &str, date: &str, resource: &str) -> String {
    format!("{method}\n\n\n{date}\n{resource}")
}

/// OSS V1 `StringToSign` for a URL-signed (presigned) GET.
fn string_to_sign_presign(expires: i64, resource: &str) -> String {
    format!("GET\n\n\n{expires}\n{resource}")
}

/// RFC 1123 date in GMT, the format OSS V1 expects.
fn http_date() -> String {
    Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// Turn a non-2xx response into a storage error, keeping a short body excerpt.
async fn ensure_success(response: reqwest::Response, op: &str, key: &str) -> Result<()> {
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body: String = response
        .text()
        .await
        .unwrap_or_default()
        .chars()
        .take(512)
        .collect();
    Err(Error::storage(format!(
        "OSS {op} {key} failed: {status} {body}"
    )))
}

/// One page of an OSS `ListObjects` response.
#[derive(Debug, Default)]
struct OssListPage {
    objects: Vec<ListedObject>,
    is_truncated: bool,
    next_marker: Option<String>,
}

/// Parse the subset of OSS `ListObjects` XML the backend needs.
fn parse_oss_list(xml: &str) -> OssListPage {
    let objects = xml_blocks(xml, "Contents")
        .into_iter()
        .filter_map(|block| {
            let key = xml_tag(&block, "Key")?;
            let size = xml_tag(&block, "Size")
                .and_then(|value| value.trim().parse::<i64>().ok())
                .unwrap_or(0);
            Some(ListedObject { key, size })
        })
        .collect();
    let is_truncated = xml_tag(xml, "IsTruncated")
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let next_marker = xml_tag(xml, "NextMarker").filter(|value| !value.is_empty());
    OssListPage {
        objects,
        is_truncated,
        next_marker,
    }
}

/// Collect the raw text of every `<tag>...</tag>` block.
fn xml_blocks(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut blocks = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        blocks.push(after[..end].to_string());
        rest = &after[end + close.len()..];
    }
    blocks
}

/// Text of the first `<tag>...</tag>` element, with entities decoded.
fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(decode_entities(&xml[start..end]))
}

/// Decode the five XML entities plus numeric character references.
fn decode_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        if let Some(semi) = rest.find(';') {
            if semi <= 12 {
                if let Some(decoded) = decode_entity(&rest[1..semi]) {
                    out.push(decoded);
                    rest = &rest[semi + 1..];
                    continue;
                }
            }
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

/// Decode one entity body (without the leading `&` and trailing `;`).
fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            if let Some(hex) = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
            {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else {
                entity
                    .strip_prefix('#')
                    .and_then(|dec| dec.parse::<u32>().ok())
                    .and_then(char::from_u32)
            }
        }
    }
}

/// Aliyun OSS storage backend.
pub struct OssStorage {
    http: Client,
    config: OssConfig,
    base: String,
    files: Mutex<HashMap<String, FileEntry>>,
    exists_cache: Mutex<HashMap<String, Instant>>,
}

impl OssStorage {
    /// Build a backend from the `CLUSTER_STORAGE_OPTIONS` JSON object.
    pub fn new(opts: &Value) -> Result<Self> {
        let config = OssConfig::parse(opts)?;
        let base = build_base_url(&config)?;
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(300))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(16)
            .tcp_nodelay(true)
            .build()?;
        Ok(OssStorage {
            http,
            config,
            base,
            files: Mutex::new(HashMap::new()),
            exists_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Full object key for a storage-relative path.
    fn object_key(&self, path: &str) -> String {
        join_key(&self.config.prefix, path)
    }

    /// Public URL of an object.
    fn object_url(&self, key: &str) -> String {
        format!("{}/{}", self.base, encode_path(key))
    }

    /// Build the OSS V1 `Authorization` header for a request.
    fn authorization(
        &self,
        method: &str,
        key: &str,
        subresources: &[(String, String)],
        date: &str,
    ) -> String {
        let resource = canonicalized_resource(&self.config.bucket, key, subresources);
        let string_to_sign = string_to_sign_v1(method, date, &resource);
        let signature = hmac_sha1_base64(&self.config.access_key_secret, &string_to_sign);
        format!("OSS {}:{signature}", self.config.access_key_id)
    }

    /// Upload an object.
    async fn put_object(&self, key: &str, body: Bytes) -> Result<()> {
        let date = http_date();
        let auth = self.authorization("PUT", key, &[], &date);
        let response = self
            .http
            .put(self.object_url(key))
            .header(header::DATE, date.as_str())
            .header(header::AUTHORIZATION, auth.as_str())
            .body(body)
            .send()
            .await?;
        ensure_success(response, "PUT", key).await
    }

    /// HEAD an object; `false` only for a definitive 404.
    async fn head_object(&self, key: &str) -> Result<bool> {
        let date = http_date();
        let auth = self.authorization("HEAD", key, &[], &date);
        let response = self
            .http
            .head(self.object_url(key))
            .header(header::DATE, date.as_str())
            .header(header::AUTHORIZATION, auth.as_str())
            .send()
            .await?;
        if response.status().is_success() {
            Ok(true)
        } else if response.status() == StatusCode::NOT_FOUND {
            Ok(false)
        } else {
            Err(Error::storage(format!(
                "OSS HEAD {key} failed: {}",
                response.status()
            )))
        }
    }

    /// DELETE an object, treating 404 as success.
    async fn delete_object(&self, key: &str) -> Result<()> {
        let date = http_date();
        let auth = self.authorization("DELETE", key, &[], &date);
        let response = self
            .http
            .delete(self.object_url(key))
            .header(header::DATE, date.as_str())
            .header(header::AUTHORIZATION, auth.as_str())
            .send()
            .await?;
        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(Error::storage(format!(
            "OSS DELETE {key} failed: {status} {body}"
        )))
    }

    /// List every object under the configured prefix (recursive, marker paged).
    async fn list_all(&self) -> Result<Vec<ListedObject>> {
        let mut objects = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut query: Vec<(String, String)> =
                vec![("max-keys".to_string(), MAX_KEYS.to_string())];
            if !self.config.prefix.is_empty() {
                query.push(("prefix".to_string(), self.config.prefix.clone()));
            }
            if let Some(marker) = &marker {
                query.push(("marker".to_string(), marker.clone()));
            }
            let date = http_date();
            let auth = self.authorization("GET", "", &query, &date);
            let query_string = query
                .iter()
                .map(|(key, value)| {
                    format!("{}={}", encode_component(key), encode_component(value))
                })
                .collect::<Vec<_>>()
                .join("&");
            let url = format!("{}/?{}", self.base, query_string);
            let response = self
                .http
                .get(url)
                .header(header::DATE, date.as_str())
                .header(header::AUTHORIZATION, auth.as_str())
                .send()
                .await?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(Error::storage(format!(
                    "OSS ListObjects failed: {status} {body}"
                )));
            }
            let body = response.text().await?;
            let page = parse_oss_list(&body);
            let last_key = page.objects.last().map(|object| object.key.clone());
            objects.extend(page.objects);
            if !page.is_truncated {
                break;
            }
            marker = page
                .next_marker
                .filter(|value| !value.is_empty())
                .or(last_key);
            if marker.is_none() {
                break;
            }
        }
        Ok(objects)
    }
}

#[async_trait]
impl Storage for OssStorage {
    async fn check(&self) -> Result<bool> {
        let key = self.object_key(".check");
        let body = Bytes::from(now_ms().to_string());
        let outcome = self.put_object(&key, body).await;
        if let Err(err) = self.delete_object(&key).await {
            warn!(%err, "failed to delete temp file");
        }
        match outcome {
            Ok(()) => Ok(true),
            Err(err) => {
                error!(%err, "storage check failed");
                Ok(false)
            }
        }
    }

    async fn write_file(&self, path: &str, content: &[u8], file_info: &FileInfo) -> Result<()> {
        let key = self.object_key(path);
        self.put_object(&key, Bytes::copy_from_slice(content))
            .await?;
        self.files.lock().await.insert(
            file_info.hash.clone(),
            FileEntry {
                size: file_info.size,
                path: file_info.path.clone(),
            },
        );
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        if let Some(seen) = self.exists_cache.lock().await.get(path) {
            if seen.elapsed() < EXISTS_TTL {
                return Ok(true);
            }
        }
        let key = self.object_key(path);
        if self.head_object(&key).await? {
            self.exists_cache
                .lock()
                .await
                .insert(path.to_string(), Instant::now());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn get_missing_files(&self, files: &[FileInfo]) -> Result<Vec<FileInfo>> {
        let mut wanted: HashMap<String, &FileInfo> =
            files.iter().map(|file| (file.hash.clone(), file)).collect();
        let known: Vec<String> = {
            let guard = self.files.lock().await;
            if guard.is_empty() {
                Vec::new()
            } else {
                guard.keys().cloned().collect()
            }
        };
        if known.is_empty() {
            for object in self.list_all().await? {
                let hash = basename(&object.key).to_string();
                let matches = wanted
                    .get(&hash)
                    .map(|file| file.size == object.size)
                    .unwrap_or(false);
                if matches {
                    self.files.lock().await.insert(
                        hash.clone(),
                        FileEntry {
                            size: object.size,
                            path: strip_prefix_key(&object.key, &self.config.prefix).to_string(),
                        },
                    );
                    wanted.remove(&hash);
                }
            }
        } else {
            for hash in known {
                wanted.remove(&hash);
            }
        }
        Ok(files
            .iter()
            .filter(|file| wanted.contains_key(&file.hash))
            .cloned()
            .collect())
    }

    async fn gc(&self, files: &[FileInfo]) -> Result<GcCounter> {
        let wanted: HashSet<String> = files.iter().map(|file| file.hash.clone()).collect();
        let mut counter = GcCounter::default();
        for object in self.list_all().await? {
            let hash = basename(&object.key);
            if wanted.contains(hash) {
                continue;
            }
            info!(path = %object.key, "delete expire file");
            self.delete_object(&object.key).await?;
            self.files.lock().await.remove(hash);
            counter.count += 1;
            counter.size += object.size.max(0) as u64;
        }
        Ok(counter)
    }

    async fn serve(&self, req: ServeRequest<'_>) -> Result<(Response, ServeStat)> {
        let key = self.object_key(req.hash_path);
        let known = self.files.lock().await.get(req.hash).cloned();
        let filename = match req.name {
            Some(name) => Some(name.to_string()),
            None => known
                .as_ref()
                .map(|entry| basename(&entry.path).to_string()),
        };
        let disposition =
            filename.map(|name| format!("attachment; filename=\"{}\"", encode_filename(&name)));
        let bytes = get_size(known.map(|entry| entry.size).unwrap_or(0), req.range).max(0) as u64;

        if self.config.proxy {
            let date = http_date();
            let auth = self.authorization("GET", &key, &[], &date);
            let mut request = self
                .http
                .get(self.object_url(&key))
                .header(header::DATE, date.as_str())
                .header(header::AUTHORIZATION, auth.as_str());
            if let Some(range) = req.range {
                request = request.header(header::RANGE, range);
            }
            let upstream = request.send().await?;
            let mut builder = Response::builder().status(upstream.status());
            for name in [
                header::CONTENT_TYPE,
                header::CONTENT_LENGTH,
                header::CONTENT_RANGE,
                header::ACCEPT_RANGES,
                header::LAST_MODIFIED,
                header::ETAG,
            ] {
                if let Some(value) = upstream.headers().get(&name) {
                    builder = builder.header(name, value.clone());
                }
            }
            let body = Body::from_stream(
                upstream
                    .bytes_stream()
                    .map(|result| result.map_err(std::io::Error::other)),
            );
            let response = builder
                .body(body)
                .map_err(|err| Error::other(format!("failed to build proxy response: {err}")))?;
            return Ok((response, ServeStat { bytes, hits: 1 }));
        }

        let expires = Utc::now().timestamp() + 60;
        let mut subresources: Vec<(String, String)> = Vec::new();
        if let Some(disposition) = &disposition {
            subresources.push((
                "response-content-disposition".to_string(),
                disposition.clone(),
            ));
        }
        let resource = canonicalized_resource(&self.config.bucket, &key, &subresources);
        let signature = hmac_sha1_base64(
            &self.config.access_key_secret,
            &string_to_sign_presign(expires, &resource),
        );
        let mut query: Vec<(String, String)> = vec![
            (
                "OSSAccessKeyId".to_string(),
                self.config.access_key_id.clone(),
            ),
            ("Expires".to_string(), expires.to_string()),
            ("Signature".to_string(), signature),
        ];
        if let Some(disposition) = disposition {
            query.push(("response-content-disposition".to_string(), disposition));
        }
        let query_string = query
            .iter()
            .map(|(name, value)| format!("{}={}", encode_component(name), encode_component(value)))
            .collect::<Vec<_>>()
            .join("&");
        let location = format!("{}?{}", self.object_url(&key), query_string);
        let response = Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, location.as_str())
            .body(Body::empty())
            .map_err(|err| Error::other(format!("failed to build redirect response: {err}")))?;
        Ok((response, ServeStat { bytes, hits: 1 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_defaults_and_default_endpoint() {
        let config = OssConfig::parse(&json!({
            "accessKeyId": "a",
            "accessKeySecret": "b",
            "bucket": "c"
        }))
        .unwrap();
        assert!(!config.internal);
        assert_eq!(config.prefix, "");
        assert!(config.proxy);
        assert!(!config.cname);
        assert!(config.endpoint.is_none());
        assert_eq!(
            build_base_url(&config).unwrap(),
            "https://c.oss-cn-hangzhou.aliyuncs.com"
        );
    }

    #[test]
    fn config_region_and_internal() {
        let config = OssConfig::parse(&json!({
            "accessKeyId": "a",
            "accessKeySecret": "b",
            "bucket": "c",
            "region": "cn-beijing",
            "internal": true
        }))
        .unwrap();
        assert_eq!(
            build_base_url(&config).unwrap(),
            "https://c.oss-cn-beijing-internal.aliyuncs.com"
        );
    }

    #[test]
    fn config_endpoint_adds_scheme() {
        let config = OssConfig::parse(&json!({
            "accessKeyId": "a",
            "accessKeySecret": "b",
            "bucket": "c",
            "endpoint": "oss.example.com"
        }))
        .unwrap();
        assert_eq!(build_base_url(&config).unwrap(), "https://oss.example.com");

        let config = OssConfig::parse(&json!({
            "accessKeyId": "a",
            "accessKeySecret": "b",
            "bucket": "c",
            "endpoint": "http://oss.example.com/"
        }))
        .unwrap();
        assert_eq!(build_base_url(&config).unwrap(), "http://oss.example.com");
    }

    #[test]
    fn config_cname_drops_bucket() {
        let config = OssConfig::parse(&json!({
            "accessKeyId": "a",
            "accessKeySecret": "b",
            "bucket": "c",
            "cname": true
        }))
        .unwrap();
        assert_eq!(
            build_base_url(&config).unwrap(),
            "https://oss-cn-hangzhou.aliyuncs.com"
        );
    }

    #[test]
    fn config_requires_credentials() {
        let error = OssConfig::parse(&json!({ "bucket": "c" })).unwrap_err();
        assert!(matches!(error, Error::Config(_)));
    }

    #[test]
    fn canonicalized_resource_sorts_subresources() {
        let subresources = vec![
            ("prefix".to_string(), "ab".to_string()),
            ("max-keys".to_string(), "1000".to_string()),
            ("marker".to_string(), "x".to_string()),
        ];
        assert_eq!(
            canonicalized_resource("bucket", "", &subresources),
            "/bucket/?marker=x&max-keys=1000&prefix=ab"
        );
        assert_eq!(
            canonicalized_resource("bucket", "ab/hash", &[]),
            "/bucket/ab/hash"
        );
    }

    #[test]
    fn string_to_sign_layout() {
        let resource = canonicalized_resource("bucket", "ab/hash", &[]);
        assert_eq!(
            string_to_sign_v1("PUT", "Wed, 01 Jan 2020 00:00:00 GMT", &resource),
            "PUT\n\n\nWed, 01 Jan 2020 00:00:00 GMT\n/bucket/ab/hash"
        );
        assert_eq!(
            string_to_sign_presign(1234567890, &resource),
            "GET\n\n\n1234567890\n/bucket/ab/hash"
        );
    }

    #[test]
    fn key_encoding_keeps_slashes() {
        assert_eq!(encode_path("ab/c d"), "ab/c%20d");
        assert_eq!(encode_component("ab/c"), "ab%2Fc");
        assert_eq!(encode_filename("a b\"c"), "a%20b%22c");
    }

    #[test]
    fn join_and_strip_prefix() {
        assert_eq!(join_key("", ".check"), ".check");
        assert_eq!(join_key("pre", "ab/hash"), "pre/ab/hash");
        assert_eq!(strip_prefix_key("pre/ab/hash", "pre"), "ab/hash");
    }

    #[test]
    fn parse_list_pages() {
        let xml = r#"<ListBucketResult><IsTruncated>true</IsTruncated><NextMarker>ab/next</NextMarker><Contents><Key>ab/one</Key><Size>1</Size></Contents><Contents><Key>ab/two</Key><Size>2</Size></Contents></ListBucketResult>"#;
        let page = parse_oss_list(xml);
        assert!(page.is_truncated);
        assert_eq!(page.next_marker.as_deref(), Some("ab/next"));
        assert_eq!(page.objects.len(), 2);
        assert_eq!(page.objects[1].size, 2);
        assert_eq!(page.objects[0].key, "ab/one");
    }

    #[test]
    fn parse_list_self_closing_marker() {
        let xml = r#"<ListBucketResult><IsTruncated>false</IsTruncated><NextMarker/><Contents><Key>ab/one</Key><Size>1</Size></Contents></ListBucketResult>"#;
        let page = parse_oss_list(xml);
        assert!(!page.is_truncated);
        assert!(page.next_marker.is_none());
        assert_eq!(page.objects.len(), 1);
    }
}
