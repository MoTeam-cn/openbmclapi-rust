//! WebDAV storage backend.
//!
//! Speaks plain WebDAV (PROPFIND / MKCOL / PUT / DELETE) over `reqwest`, so it
//! also backs the AList variant. Certificate validation is disabled to match
//! the Node agent, which passes `rejectUnauthorized: false`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{info, trace, warn};

use crate::error::{Error, Result};
use crate::types::{FileInfo, GcCounter};
use crate::util::get_size;

use super::{ServeRequest, ServeStat, Storage};

const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:resourcetype/>
    <D:getcontentlength/>
    <D:getlastmodified/>
  </D:prop>
</D:propfind>"#;

/// A single entry of a WebDAV directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavEntry {
    /// Fully qualified URL of the entry.
    pub href: String,
    /// Last path segment, used as the content hash.
    pub name: String,
    pub size: i64,
    pub is_dir: bool,
}

/// Thin WebDAV client.
#[derive(Clone)]
pub struct WebdavClient {
    http: reqwest::Client,
    base: String,
    username: Option<String>,
    password: Option<String>,
}

impl WebdavClient {
    pub fn new(base: &str, username: Option<String>, password: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .build()?;
        Ok(WebdavClient {
            http,
            base: base.trim_end_matches('/').to_string(),
            username,
            password,
        })
    }

    /// Build a request with basic auth applied when configured.
    pub fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.http.request(method, url);
        if let (Some(user), Some(pass)) = (&self.username, &self.password) {
            builder = builder.basic_auth(user, Some(pass));
        }
        builder
    }

    /// Resolve a path against the WebDAV root.
    pub fn url(&self, path: &str) -> String {
        join_url(&self.base, path)
    }

    /// URL handed to the client for a download.
    pub fn download_link(&self, path: &str) -> String {
        self.url(path)
    }

    /// PROPFIND with the given depth, returning the parsed entries.
    pub async fn propfind(&self, url: &str, depth: u32) -> Result<Vec<DavEntry>> {
        let response = self
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), url)
            .header("Depth", depth.to_string())
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(PROPFIND_BODY)
            .send()
            .await?;
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Err(Error::NotFound);
        }
        if !status.is_success() && status != StatusCode::MULTI_STATUS {
            return Err(Error::Status {
                status: status.as_u16(),
                url: url.to_string(),
            });
        }
        let body = response.text().await?;
        Ok(parse_multistatus(&body))
    }

    /// Whether a path exists.
    pub async fn exists(&self, url: &str) -> Result<bool> {
        match self.propfind(url, 0).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound) => Ok(false),
            Err(Error::Status { status: 404, .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// MKCOL, creating every missing parent segment.
    pub async fn create_directory(&self, url: &str) -> Result<()> {
        let root = self.base.trim_end_matches('/').to_string();
        let relative = url.strip_prefix(&root).unwrap_or(url);
        let mut current = root;
        for segment in relative.split('/').filter(|s| !s.is_empty()) {
            current = format!("{current}/{}", encode_segment(segment));
            let response = self
                .request(reqwest::Method::from_bytes(b"MKCOL").unwrap(), &current)
                .send()
                .await?;
            let status = response.status();
            // 405 means it already exists, which is fine.
            if !status.is_success() && status.as_u16() != 405 {
                return Err(Error::storage(format!(
                    "MKCOL {current} failed with {status}"
                )));
            }
        }
        Ok(())
    }

    pub async fn put(&self, url: &str, content: &[u8]) -> Result<()> {
        let response = self
            .request(reqwest::Method::PUT, url)
            .body(content.to_vec())
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Status {
                status: status.as_u16(),
                url: url.to_string(),
            });
        }
        Ok(())
    }

    pub async fn delete(&self, url: &str) -> Result<()> {
        let response = self.request(reqwest::Method::DELETE, url).send().await?;
        let status = response.status();
        if !status.is_success() && status != StatusCode::NOT_FOUND {
            return Err(Error::Status {
                status: status.as_u16(),
                url: url.to_string(),
            });
        }
        Ok(())
    }
}

/// Join a base URL with a `/`-separated path, percent-encoding each segment.
pub fn join_url(base: &str, path: &str) -> String {
    let mut out = base.trim_end_matches('/').to_string();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        out.push('/');
        out.push_str(&encode_segment(segment));
    }
    out
}

fn encode_segment(segment: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    const KEEP: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    utf8_percent_encode(segment, KEEP).to_string()
}

/// Parse a `207 Multi-Status` document into flat entries.
pub fn parse_multistatus(body: &str) -> Vec<DavEntry> {
    let mut reader = Reader::from_str(body);
    let mut entries = Vec::new();
    let mut current: Option<DavEntry> = None;
    let mut text_target: Option<&'static str> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let local = local_name(event.name().as_ref());
                match local.as_str() {
                    "response" => {
                        current = Some(DavEntry {
                            href: String::new(),
                            name: String::new(),
                            size: 0,
                            is_dir: false,
                        });
                    }
                    "href" => text_target = Some("href"),
                    "getcontentlength" => text_target = Some("size"),
                    _ => {}
                }
            }
            Ok(Event::Empty(event)) => {
                if local_name(event.name().as_ref()) == "collection" {
                    if let Some(entry) = current.as_mut() {
                        entry.is_dir = true;
                    }
                }
            }
            Ok(Event::Text(event)) => {
                if let (Some(entry), Some(target)) = (current.as_mut(), text_target) {
                    let text = String::from_utf8_lossy(event.as_ref()).trim().to_string();
                    match target {
                        "href" => entry.href = text,
                        "size" => entry.size = text.parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(event)) => {
                let local = local_name(event.name().as_ref());
                if local == "href" || local == "getcontentlength" {
                    text_target = None;
                } else if local == "response" {
                    if let Some(mut entry) = current.take() {
                        if !entry.href.is_empty() {
                            let trimmed = entry.href.trim_end_matches('/');
                            entry.name = trimmed.rsplit('/').next().unwrap_or(trimmed).to_string();
                            entries.push(entry);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!(error = %e, "failed to parse WebDAV multistatus");
                break;
            }
            _ => {}
        }
    }
    entries
}

fn local_name(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    match text.rsplit_once(':') {
        Some((_, local)) => local.to_ascii_lowercase(),
        None => text.to_ascii_lowercase(),
    }
}

/// Simple in-memory TTL set used for the existence cache.
#[derive(Default)]
struct TtlCache {
    entries: Mutex<HashMap<String, Instant>>,
    ttl: Duration,
}

impl TtlCache {
    fn new(ttl: Duration) -> Self {
        TtlCache {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    async fn contains(&self, key: &str) -> bool {
        let guard = self.entries.lock().await;
        guard.get(key).is_some_and(|at| at.elapsed() < self.ttl)
    }

    async fn insert(&self, key: &str) {
        self.entries
            .lock()
            .await
            .insert(key.to_string(), Instant::now());
    }

    async fn remove(&self, key: &str) {
        self.entries.lock().await.remove(key);
    }
}

/// WebDAV-backed storage.
pub struct WebdavStorage {
    client: WebdavClient,
    base_path: String,
    files: Mutex<HashMap<String, (i64, String)>>,
    empty_files: Mutex<HashSet<String>>,
    exists_cache: TtlCache,
}

impl WebdavStorage {
    pub fn new(opts: &Value) -> Result<Self> {
        let url = string_field(opts, "url")
            .ok_or_else(|| Error::Config("webdav: url is required".into()))?;
        let base_path = string_field(opts, "basePath").unwrap_or_default();
        let client = WebdavClient::new(
            &url,
            string_field(opts, "username"),
            string_field(opts, "password"),
        )?;
        Ok(WebdavStorage {
            client,
            base_path,
            files: Mutex::new(HashMap::new()),
            empty_files: Mutex::new(HashSet::new()),
            exists_cache: TtlCache::new(Duration::from_secs(3600)),
        })
    }

    pub(crate) fn remote(&self, key: &str) -> String {
        self.client.url(&format!(
            "{}/{}",
            self.base_path.trim_matches('/'),
            key.trim_start_matches('/')
        ))
    }

    /// Size of an object the agent has already written, if known.
    pub(crate) async fn known_size(&self, hash: &str) -> Option<i64> {
        self.files.lock().await.get(hash).map(|(size, _)| *size)
    }

    /// Whether the object was stored as a zero-byte placeholder.
    pub(crate) async fn is_empty_file(&self, key: &str) -> bool {
        self.empty_files.lock().await.contains(key)
    }

    /// Underlying WebDAV client.
    pub(crate) fn client(&self) -> &WebdavClient {
        &self.client
    }
}

#[async_trait]
impl Storage for WebdavStorage {
    async fn init(&self) -> Result<()> {
        let root = self.remote("");
        if !self.client.exists(&root).await? {
            info!(path = %self.base_path, "create webdav base path");
            self.client.create_directory(&root).await?;
        }
        Ok(())
    }

    async fn check(&self) -> Result<bool> {
        let probe = self.remote(".check");
        let outcome = self.client.put(&probe, now_string().as_bytes()).await;
        let _ = self.client.delete(&probe).await;
        match outcome {
            Ok(()) => Ok(true),
            Err(e) => {
                warn!(error = %e, "storage check failed");
                Ok(false)
            }
        }
    }

    async fn write_file(&self, path: &str, content: &[u8], file_info: &FileInfo) -> Result<()> {
        if content.is_empty() {
            self.empty_files.lock().await.insert(path.to_string());
            return Ok(());
        }
        let url = self.remote(path);
        self.client.put(&url, content).await?;
        self.files.lock().await.insert(
            file_info.hash.clone(),
            (content.len() as i64, file_info.path.clone()),
        );
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        if self.exists_cache.contains(path).await {
            return Ok(true);
        }
        let found = self.client.exists(&self.remote(path)).await?;
        if found {
            self.exists_cache.insert(path).await;
        }
        Ok(found)
    }

    async fn get_missing_files(&self, files: &[FileInfo]) -> Result<Vec<FileInfo>> {
        let mut remote: HashMap<String, FileInfo> =
            files.iter().map(|f| (f.hash.clone(), f.clone())).collect();

        {
            let known = self.files.lock().await;
            if !known.is_empty() {
                for hash in known.keys() {
                    remote.remove(hash);
                }
                return Ok(remote.into_values().collect());
            }
        }

        let root = self.remote("");
        let mut queue = vec![root];
        let mut checked = 0usize;
        while let Some(dir) = queue.pop() {
            let entries = self.client.propfind(&dir, 1).await?;
            checked += 1;
            trace!(dir = %dir, checked, "scanning webdav directory");
            for entry in entries {
                if entry.is_dir {
                    queue.push(entry.href.clone());
                    continue;
                }
                if let Some(file) = remote.get(&entry.name) {
                    if file.size == entry.size {
                        self.files
                            .lock()
                            .await
                            .insert(entry.name.clone(), (entry.size, entry.href.clone()));
                        remote.remove(&entry.name);
                    }
                }
            }
        }
        Ok(remote.into_values().collect())
    }

    async fn gc(&self, files: &[FileInfo]) -> Result<GcCounter> {
        let wanted: HashSet<String> = files.iter().map(|f| f.hash.clone()).collect();
        let mut counter = GcCounter::default();
        let root = self.remote("");
        let mut queue = vec![root];
        while let Some(dir) = queue.pop() {
            let entries = self.client.propfind(&dir, 1).await?;
            for entry in entries {
                if entry.is_dir {
                    queue.push(entry.href.clone());
                    continue;
                }
                if !wanted.contains(&entry.name) {
                    info!(path = %entry.href, "delete expire file");
                    if self.client.delete(&entry.href).await.is_ok() {
                        self.files.lock().await.remove(&entry.name);
                        self.exists_cache.remove(&entry.name).await;
                        counter.count += 1;
                        counter.size += entry.size.max(0) as u64;
                    }
                }
            }
        }
        Ok(counter)
    }

    async fn serve(&self, req: ServeRequest<'_>) -> Result<(Response, ServeStat)> {
        if self.is_empty_file(req.hash_path).await {
            return Ok((empty_ok(), ServeStat { bytes: 0, hits: 1 }));
        }
        let url = self.client.download_link(&self.remote(req.hash_path));
        let size = get_size(self.known_size(req.hash).await.unwrap_or(0), req.range);
        Ok((
            redirect(url),
            ServeStat {
                bytes: size.max(0) as u64,
                hits: 1,
            },
        ))
    }
}

fn string_field(opts: &Value, key: &str) -> Option<String> {
    opts.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn now_string() -> String {
    crate::util::now_ms().to_string()
}

/// 302 redirect response.
pub(crate) fn redirect(url: String) -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, url)
        .body(Body::empty())
        .expect("valid redirect response")
}

/// Empty 200 response.
pub(crate) fn empty_ok() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())
        .expect("valid empty response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multistatus() {
        let body = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/cache/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/cache/ab/abcdef</D:href>
    <D:propstat><D:prop><D:resourcetype/><D:getcontentlength>123</D:getcontentlength></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
</D:multistatus>"#;
        let entries = parse_multistatus(body);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "cache");
        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].name, "abcdef");
        assert_eq!(entries[1].size, 123);
    }

    #[test]
    fn joins_urls() {
        assert_eq!(
            join_url("http://host/dav/", "ab/cd ef"),
            "http://host/dav/ab/cd%20ef"
        );
    }

    #[test]
    fn hash_key_uses_slash() {
        assert_eq!(crate::util::hash_to_filename("abcdef"), "ab/abcdef");
    }
}
