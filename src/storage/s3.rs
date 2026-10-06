//! MinIO / S3-compatible object storage.
//!
//! Port of `src/storage/minio.storage.ts`. The port cannot use the MinIO
//! SDK, so requests are signed with AWS Signature Version 4 and objects are
//! addressed path-style (`<scheme>://<authority>/<bucket>/<key>`).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use url::Url;

use crate::error::{Error, Result};
use crate::types::{FileInfo, GcCounter};
use crate::util::{get_size, now_ms};

use super::{ServeRequest, ServeStat, Storage};

/// Characters left unescaped by AWS `UriEncode`; everything else is encoded.
const AWS_UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// [`AWS_UNRESERVED`] plus `/`, so object paths keep their separators.
const AWS_UNRESERVED_PATH: &AsciiSet = &AWS_UNRESERVED.remove(b'/');

/// `encodeURIComponent` semantics, used for `content-disposition` filenames.
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

/// Connection parameters for one S3-compatible endpoint.
#[derive(Debug, Clone)]
struct Endpoint {
    scheme: String,
    host: String,
    port: Option<u16>,
    access_key: String,
    secret_key: String,
    region: String,
}

impl Endpoint {
    /// Parse a storage URL: credentials in the userinfo, `region` in the query.
    fn parse(raw: &str, default_region: Option<&str>) -> Result<Self> {
        let url = Url::parse(raw)
            .map_err(|e| Error::Config(format!("invalid storage url {raw:?}: {e}")))?;
        let host = match url.host() {
            Some(url::Host::Ipv6(addr)) => format!("[{addr}]"),
            Some(url::Host::Ipv4(addr)) => addr.to_string(),
            Some(url::Host::Domain(domain)) => domain.to_string(),
            None => return Err(Error::Config(format!("storage url {raw:?} has no host"))),
        };
        let region = url
            .query_pairs()
            .find(|(key, _)| key.as_ref() == "region")
            .map(|(_, value)| value.into_owned())
            .or_else(|| default_region.map(str::to_string))
            .unwrap_or_else(|| "us-east-1".to_string());
        Ok(Endpoint {
            scheme: url.scheme().to_string(),
            host,
            port: url.port(),
            access_key: url.username().to_string(),
            secret_key: url.password().unwrap_or_default().to_string(),
            region,
        })
    }

    /// `host[:port]`, matching the HTTP `Host` header.
    fn authority(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{}", self.host, port),
            None => self.host.clone(),
        }
    }

    /// `scheme://host[:port]`.
    fn base_url(&self) -> String {
        format!("{}://{}", self.scheme, self.authority())
    }
}

/// Split `<bucket>/<prefix...>` out of a storage URL path.
fn split_bucket_prefix(raw: &str) -> Result<(String, String)> {
    let url =
        Url::parse(raw).map_err(|e| Error::Config(format!("invalid storage url {raw:?}: {e}")))?;
    let mut segments = url.path().split('/').filter(|segment| !segment.is_empty());
    let bucket = segments
        .next()
        .ok_or_else(|| Error::Config(format!("storage url {raw:?} has no bucket")))?
        .to_string();
    let prefix = segments.collect::<Vec<_>>().join("/");
    Ok((bucket, prefix))
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
    percent_encode(key.as_bytes(), AWS_UNRESERVED_PATH).to_string()
}

/// Percent-encode a single query component.
fn encode_component(value: &str) -> String {
    percent_encode(value.as_bytes(), AWS_UNRESERVED).to_string()
}

/// Percent-encode a filename the way `encodeURIComponent` does.
fn encode_filename(name: &str) -> String {
    percent_encode(name.as_bytes(), URI_COMPONENT).to_string()
}

/// Lowercase hex SHA-256 digest.
fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// HMAC-SHA256 over `data` with `key`.
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// AWS SigV4 canonical query string: encoded, then sorted.
fn canonical_query_string(query: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(key, value)| (encode_component(key), encode_component(value)))
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// AWS SigV4 canonical headers plus the matching `SignedHeaders` list.
fn canonical_headers(headers: &[(String, String)]) -> (String, String) {
    let mut sorted: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    sorted.sort();
    let block: String = sorted
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect();
    let signed = sorted
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>()
        .join(";");
    (block, signed)
}

/// Derive the SigV4 signing key for `date`/`region`/`service`.
fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// Sign a request with AWS Signature Version 4 and return the headers to send.
fn sign_v4(
    endpoint: &Endpoint,
    method: &str,
    canonical_uri: &str,
    query: &[(String, String)],
    payload_hash: &str,
    now: DateTime<Utc>,
) -> Vec<(String, String)> {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let scope = format!("{date}/{}/s3/aws4_request", endpoint.region);
    let headers = vec![
        ("host".to_string(), endpoint.authority()),
        ("x-amz-content-sha256".to_string(), payload_hash.to_string()),
        ("x-amz-date".to_string(), amz_date.clone()),
    ];
    let (header_block, signed_headers) = canonical_headers(&headers);
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{}\n{header_block}\n{signed_headers}\n{payload_hash}",
        canonical_query_string(query),
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes()),
    );
    let signature = hex::encode(hmac_sha256(
        &signing_key(&endpoint.secret_key, &date, &endpoint.region, "s3"),
        string_to_sign.as_bytes(),
    ));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        endpoint.access_key,
    );
    vec![
        ("host".to_string(), endpoint.authority()),
        ("x-amz-content-sha256".to_string(), payload_hash.to_string()),
        ("x-amz-date".to_string(), amz_date),
        ("authorization".to_string(), authorization),
    ]
}

/// Build a presigned `GET` URL with the given expiry in seconds.
fn presign_get(
    endpoint: &Endpoint,
    canonical_uri: &str,
    expires: u64,
    disposition: Option<&str>,
) -> String {
    let now = Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let scope = format!("{date}/{}/s3/aws4_request", endpoint.region);
    let mut query = vec![
        (
            "X-Amz-Algorithm".to_string(),
            "AWS4-HMAC-SHA256".to_string(),
        ),
        (
            "X-Amz-Credential".to_string(),
            format!("{}/{}", endpoint.access_key, scope),
        ),
        ("X-Amz-Date".to_string(), amz_date.clone()),
        ("X-Amz-Expires".to_string(), expires.to_string()),
        ("X-Amz-SignedHeaders".to_string(), "host".to_string()),
    ];
    if let Some(disposition) = disposition {
        query.push((
            "response-content-disposition".to_string(),
            disposition.to_string(),
        ));
    }
    let canonical_query = canonical_query_string(&query);
    let header_block = format!("host:{}\n", endpoint.authority());
    let canonical_request =
        format!("GET\n{canonical_uri}\n{canonical_query}\n{header_block}\nhost\nUNSIGNED-PAYLOAD");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes()),
    );
    let signature = hex::encode(hmac_sha256(
        &signing_key(&endpoint.secret_key, &date, &endpoint.region, "s3"),
        string_to_sign.as_bytes(),
    ));
    format!(
        "{}{}?{canonical_query}&X-Amz-Signature={signature}",
        endpoint.base_url(),
        canonical_uri,
    )
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
        "S3 {op} {key} failed: {status} {body}"
    )))
}

/// One page of a `ListObjectsV2` response.
#[derive(Debug, Default)]
struct S3ListPage {
    objects: Vec<ListedObject>,
    is_truncated: bool,
    next_token: Option<String>,
}

/// Parse the subset of `ListObjectsV2` XML the backend needs.
fn parse_s3_list(xml: &str) -> S3ListPage {
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
    let next_token = xml_tag(xml, "NextContinuationToken").filter(|value| !value.is_empty());
    S3ListPage {
        objects,
        is_truncated,
        next_token,
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

/// MinIO / S3-compatible storage backend.
pub struct MinioStorage {
    http: Client,
    public: Endpoint,
    internal: Endpoint,
    bucket: String,
    prefix: String,
    files: Mutex<HashMap<String, FileEntry>>,
    exists_cache: Mutex<HashMap<String, Instant>>,
}

impl MinioStorage {
    /// Build a backend from the `CLUSTER_STORAGE_OPTIONS` JSON object.
    pub fn new(opts: &Value) -> Result<Self> {
        let raw = opts
            .get("url")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::Config("minio storage requires a non-empty \"url\"".into()))?;
        let public = Endpoint::parse(raw, None)?;
        let internal = match opts.get("internalUrl").and_then(Value::as_str) {
            Some(value) if !value.is_empty() => Endpoint::parse(value, Some(&public.region))?,
            _ => public.clone(),
        };
        let (bucket, prefix) = split_bucket_prefix(raw)?;
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(16)
            .tcp_nodelay(true)
            .build()?;
        Ok(MinioStorage {
            http,
            public,
            internal,
            bucket,
            prefix,
            files: Mutex::new(HashMap::new()),
            exists_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Full object key for a storage-relative path.
    fn object_key(&self, path: &str) -> String {
        join_key(&self.prefix, path)
    }

    /// Canonical URI of an object.
    fn object_uri(&self, key: &str) -> String {
        format!("/{}/{}", self.bucket, encode_path(key))
    }

    /// Canonical URI of the bucket, used for listings.
    fn bucket_uri(&self) -> String {
        format!("/{}", self.bucket)
    }

    /// Send one SigV4-signed request.
    async fn send_signed(
        &self,
        endpoint: &Endpoint,
        method: reqwest::Method,
        uri: &str,
        query: &[(String, String)],
        body: Option<Bytes>,
    ) -> Result<reqwest::Response> {
        let payload_hash = match &body {
            Some(bytes) => sha256_hex(bytes),
            None => sha256_hex(b""),
        };
        let headers = sign_v4(
            endpoint,
            method.as_str(),
            uri,
            query,
            &payload_hash,
            Utc::now(),
        );
        let url = if query.is_empty() {
            format!("{}{}", endpoint.base_url(), uri)
        } else {
            format!(
                "{}{}?{}",
                endpoint.base_url(),
                uri,
                canonical_query_string(query)
            )
        };
        let mut request = self.http.request(method, url);
        for (name, value) in &headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(bytes) = body {
            request = request.body(bytes);
        }
        Ok(request.send().await?)
    }

    /// Upload an object through the given endpoint.
    async fn put_object(&self, endpoint: &Endpoint, key: &str, body: Bytes) -> Result<()> {
        let uri = self.object_uri(key);
        let response = self
            .send_signed(endpoint, reqwest::Method::PUT, &uri, &[], Some(body))
            .await?;
        ensure_success(response, "PUT", key).await
    }

    /// HEAD an object; `false` only for a definitive 404.
    async fn head_object(&self, endpoint: &Endpoint, key: &str) -> Result<bool> {
        let uri = self.object_uri(key);
        let response = self
            .send_signed(endpoint, reqwest::Method::HEAD, &uri, &[], None)
            .await?;
        if response.status().is_success() {
            Ok(true)
        } else if response.status() == StatusCode::NOT_FOUND {
            Ok(false)
        } else {
            Err(Error::storage(format!(
                "S3 HEAD {key} failed: {}",
                response.status()
            )))
        }
    }

    /// DELETE an object, treating 404 as success.
    async fn delete_object(&self, endpoint: &Endpoint, key: &str) -> Result<()> {
        let uri = self.object_uri(key);
        let response = self
            .send_signed(endpoint, reqwest::Method::DELETE, &uri, &[], None)
            .await?;
        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(Error::storage(format!(
            "S3 DELETE {key} failed: {status} {body}"
        )))
    }

    /// List every object under the configured prefix (recursive).
    async fn list_all(&self, endpoint: &Endpoint) -> Result<Vec<ListedObject>> {
        let mut objects = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query: Vec<(String, String)> = vec![
                ("list-type".to_string(), "2".to_string()),
                ("max-keys".to_string(), "1000".to_string()),
            ];
            if !self.prefix.is_empty() {
                query.push(("prefix".to_string(), self.prefix.clone()));
            }
            if let Some(token) = &token {
                query.push(("continuation-token".to_string(), token.clone()));
            }
            let response = self
                .send_signed(
                    endpoint,
                    reqwest::Method::GET,
                    &self.bucket_uri(),
                    &query,
                    None,
                )
                .await?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(Error::storage(format!(
                    "S3 ListObjectsV2 failed: {status} {body}"
                )));
            }
            let body = response.text().await?;
            let page = parse_s3_list(&body);
            objects.extend(page.objects);
            if !page.is_truncated {
                break;
            }
            match page.next_token {
                Some(next) if !next.is_empty() => token = Some(next),
                _ => break,
            }
        }
        Ok(objects)
    }
}

#[async_trait]
impl Storage for MinioStorage {
    async fn check(&self) -> Result<bool> {
        let key = self.object_key(".check");
        let body = Bytes::from(now_ms().to_string());
        let outcome = async {
            self.put_object(&self.internal, &key, body.clone()).await?;
            self.put_object(&self.public, &key, body.clone()).await?;
            Ok::<(), Error>(())
        }
        .await;
        if let Err(err) = self.delete_object(&self.internal, &key).await {
            warn!(%err, "failed to delete temp file");
        }
        if let Err(err) = self.delete_object(&self.public, &key).await {
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
        self.put_object(&self.internal, &key, Bytes::copy_from_slice(content))
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
        if self.head_object(&self.internal, &key).await? {
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
            for object in self.list_all(&self.internal).await? {
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
                            path: strip_prefix_key(&object.key, &self.prefix).to_string(),
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
        for object in self.list_all(&self.internal).await? {
            let hash = basename(&object.key);
            if wanted.contains(hash) {
                continue;
            }
            info!(path = %object.key, "delete expire file");
            self.delete_object(&self.internal, &object.key).await?;
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
        let location = presign_get(
            &self.public,
            &self.object_uri(&key),
            60,
            disposition.as_deref(),
        );
        let bytes = get_size(known.map(|entry| entry.size).unwrap_or(0), req.range).max(0) as u64;
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

    #[test]
    fn join_key_variants() {
        assert_eq!(join_key("", ".check"), ".check");
        assert_eq!(join_key("pre", ".check"), "pre/.check");
        assert_eq!(join_key("pre/", "/ab/hash"), "pre/ab/hash");
    }

    #[test]
    fn strip_prefix_variants() {
        assert_eq!(strip_prefix_key("pre/ab/hash", "pre"), "ab/hash");
        assert_eq!(strip_prefix_key("ab/hash", ""), "ab/hash");
        assert_eq!(strip_prefix_key("other/ab/hash", "pre"), "other/ab/hash");
    }

    #[test]
    fn encoding_rules() {
        assert_eq!(encode_path("ab/cd ef"), "ab/cd%20ef");
        assert_eq!(encode_component("a/b"), "a%2Fb");
        assert_eq!(encode_filename("a b\"c"), "a%20b%22c");
        assert_eq!(encode_filename("keep-_.!~*'()"), "keep-_.!~*'()");
    }

    #[test]
    fn endpoint_parsing() {
        let endpoint = Endpoint::parse(
            "https://key:secret@minio.example.com:9000/bucket/prefix?region=us-west-1",
            None,
        )
        .unwrap();
        assert_eq!(endpoint.scheme, "https");
        assert_eq!(endpoint.host, "minio.example.com");
        assert_eq!(endpoint.port, Some(9000));
        assert_eq!(endpoint.access_key, "key");
        assert_eq!(endpoint.secret_key, "secret");
        assert_eq!(endpoint.region, "us-west-1");
        assert_eq!(endpoint.authority(), "minio.example.com:9000");
        assert_eq!(endpoint.base_url(), "https://minio.example.com:9000");

        let (bucket, prefix) =
            split_bucket_prefix("https://key:secret@minio.example.com:9000/bucket/a/b").unwrap();
        assert_eq!(bucket, "bucket");
        assert_eq!(prefix, "a/b");
    }

    #[test]
    fn endpoint_default_region_and_port() {
        let endpoint = Endpoint::parse("http://127.0.0.1/bucket", None).unwrap();
        assert_eq!(endpoint.region, "us-east-1");
        assert_eq!(endpoint.port, None);
        assert_eq!(endpoint.authority(), "127.0.0.1");
    }

    #[test]
    fn canonical_query_is_sorted_and_encoded() {
        let query = vec![
            ("prefix".to_string(), "a b".to_string()),
            ("list-type".to_string(), "2".to_string()),
        ];
        assert_eq!(canonical_query_string(&query), "list-type=2&prefix=a%20b");
    }

    #[test]
    fn canonical_headers_layout() {
        let headers = vec![
            ("x-amz-date".to_string(), "20200101T000000Z".to_string()),
            ("host".to_string(), "example.com".to_string()),
        ];
        let (block, signed) = canonical_headers(&headers);
        assert_eq!(block, "host:example.com\nx-amz-date:20200101T000000Z\n");
        assert_eq!(signed, "host;x-amz-date");
    }

    #[test]
    fn parse_list_extracts_objects() {
        let xml = r#"<?xml version="1.0"?><ListBucketResult><IsTruncated>true</IsTruncated><Contents><Key>ab/abc&amp;d</Key><Size>12</Size></Contents><NextContinuationToken>tok</NextContinuationToken></ListBucketResult>"#;
        let page = parse_s3_list(xml);
        assert!(page.is_truncated);
        assert_eq!(page.next_token.as_deref(), Some("tok"));
        assert_eq!(
            page.objects,
            vec![ListedObject {
                key: "ab/abc&d".to_string(),
                size: 12
            }]
        );
    }

    #[test]
    fn parse_list_without_truncation() {
        let xml = r#"<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>ab/one</Key><Size>1</Size></Contents></ListBucketResult>"#;
        let page = parse_s3_list(xml);
        assert!(!page.is_truncated);
        assert!(page.next_token.is_none());
        assert_eq!(page.objects.len(), 1);
    }

    #[test]
    fn presign_shape() {
        let endpoint =
            Endpoint::parse("https://key:secret@minio.example.com:9000/bucket", None).unwrap();
        let url = presign_get(
            &endpoint,
            "/bucket/ab/hash",
            60,
            Some("attachment; filename=\"x\""),
        );
        assert!(url.starts_with("https://minio.example.com:9000/bucket/ab/hash?"));
        assert!(url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
        assert!(url.contains("X-Amz-SignedHeaders=host"));
        assert!(url.contains("X-Amz-Signature="));
        assert!(url.contains("response-content-disposition="));
    }
}
