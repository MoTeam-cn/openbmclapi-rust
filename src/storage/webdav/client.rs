//! Thin WebDAV client.
//!
//! Wraps the verbs the backend needs (PROPFIND / MKCOL / PUT / DELETE) and owns
//! the URL encoding rules, so callers never build WebDAV URLs by hand.

use std::time::Duration;

use axum::http::{header, StatusCode};

use crate::error::{Error, Result};

use super::xml::{parse_multistatus, DavEntry};

const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:resourcetype/>
    <D:getcontentlength/>
    <D:getlastmodified/>
  </D:prop>
</D:propfind>"#;

/// Thin WebDAV client.
#[derive(Clone)]
pub struct WebdavClient {
    http: reqwest::Client,
    base: String,
    username: Option<String>,
    password: Option<String>,
}

impl WebdavClient {
    /// Build a client rooted at `base`; TLS validation stays off to match the Node agent.
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

    /// PUT `content` to `url`.
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

    /// DELETE `url`; a missing object is not an error.
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

#[cfg(test)]
#[path = "client_test.rs"]
mod tests;
