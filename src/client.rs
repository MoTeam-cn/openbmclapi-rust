//! HTTP client for the BMCLAPI master.
//!
//! Adds the bearer token to whitelisted hosts and mirrors the small response
//! cache the Node agent uses for `files` and `configuration`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::Mutex;
use tracing::debug;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::token::TokenManager;

/// Hosts that receive the `Authorization` header.
const DEFAULT_WHITELIST: [&str; 2] = ["localhost", "bangbang93.com"];

/// A fully buffered HTTP response.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub status: StatusCode,
    pub body: Bytes,
    pub headers: HeaderMap,
}

impl Fetched {
    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }
}

#[derive(Clone)]
pub struct BmclapiClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    base: String,
    token: TokenManager,
    whitelist: Vec<String>,
    cache: Mutex<HashMap<String, Fetched>>,
}

impl BmclapiClient {
    pub fn new(config: &Config, token: TokenManager) -> Result<Self> {
        let ua = format!("openbmclapi-cluster/{}", crate::VERSION);
        let http = reqwest::Client::builder()
            .user_agent(&ua)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(16)
            .tcp_nodelay(true)
            .build()?;

        let base = config.bmclapi_base.trim_end_matches('/').to_string();
        let mut whitelist: Vec<String> = DEFAULT_WHITELIST.iter().map(|s| s.to_string()).collect();
        whitelist.push(base.clone());

        Ok(BmclapiClient {
            inner: Arc::new(Inner {
                http,
                base,
                token,
                whitelist,
                cache: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub fn base(&self) -> &str {
        &self.inner.base
    }

    pub fn token_manager(&self) -> &TokenManager {
        &self.inner.token
    }

    fn build_url(&self, path: &str, query: &[(&str, String)]) -> Result<url::Url> {
        let joined = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}/{}", self.inner.base, path.trim_start_matches('/'))
        };
        let mut url = url::Url::parse(&joined)?;
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        Ok(url)
    }

    async fn authorize(
        &self,
        request: reqwest::RequestBuilder,
        url: &url::Url,
    ) -> Result<reqwest::RequestBuilder> {
        let target = url.as_str();
        if self
            .inner
            .whitelist
            .iter()
            .any(|domain| target.contains(domain.as_str()))
        {
            let token = self.inner.token.get_token().await?;
            return Ok(request.bearer_auth(token));
        }
        Ok(request)
    }

    /// Perform a buffered GET. When `use_cache` is set the response is stored
    /// under the resolved URL and reused, mirroring `got`'s `cache` option.
    pub async fn get_bytes(
        &self,
        path: &str,
        query: &[(&str, String)],
        use_cache: bool,
    ) -> Result<Fetched> {
        let url = self.build_url(path, query)?;
        let key = url.to_string();
        if use_cache {
            if let Some(hit) = self.inner.cache.lock().await.get(&key).cloned() {
                return Ok(hit);
            }
        }
        let request = self
            .authorize(self.inner.http.get(url.clone()), &url)
            .await?;
        let response = request.send().await?;
        let fetched = buffer(response).await?;
        if use_cache {
            self.inner.cache.lock().await.insert(key, fetched.clone());
        }
        Ok(fetched)
    }

    /// Perform a GET and decode a JSON body, failing on non-2xx.
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        use_cache: bool,
    ) -> Result<T> {
        let fetched = self.get_bytes(path, query, use_cache).await?;
        if !fetched.is_success() {
            return Err(Error::Status {
                status: fetched.status.as_u16(),
                url: path.to_string(),
            });
        }
        Ok(serde_json::from_slice(&fetched.body)?)
    }

    /// Perform a streaming GET (used for file downloads).
    pub async fn get_stream(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<reqwest::Response> {
        let url = self.build_url(path, query)?;
        let request = self
            .authorize(self.inner.http.get(url.clone()), &url)
            .await?;
        let response = request.send().await?;
        Ok(response)
    }

    /// Perform a streaming GET against an absolute URL (no auth injection).
    pub async fn get_absolute(&self, url: &str) -> Result<reqwest::Response> {
        let parsed = url::Url::parse(url)?;
        let response = self.inner.http.get(parsed).send().await?;
        Ok(response)
    }

    /// POST a JSON body and buffer the response.
    pub async fn post_json(&self, path: &str, body: &serde_json::Value) -> Result<Fetched> {
        let url = self.build_url(path, &[])?;
        let request = self
            .authorize(self.inner.http.post(url.clone()), &url)
            .await?;
        let response = request.json(body).send().await?;
        buffer(response).await
    }

    /// POST a JSON body, ignoring the response body.
    pub async fn post_json_ok(&self, path: &str, body: &serde_json::Value) -> Result<()> {
        let fetched = self.post_json(path, body).await?;
        if !fetched.is_success() {
            debug!(status = %fetched.status, path, "master rejected request");
            return Err(Error::Status {
                status: fetched.status.as_u16(),
                url: path.to_string(),
            });
        }
        Ok(())
    }

    /// Convenience wrapper for the `report` endpoint.
    pub async fn report(&self, body: serde_json::Value) -> Result<()> {
        self.post_json_ok("openbmclapi/report", &body).await
    }
}

async fn buffer(response: reqwest::Response) -> Result<Fetched> {
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await?;
    Ok(Fetched {
        status,
        body,
        headers,
    })
}

/// Helper so callers can build typed query slices ergonomically.
pub fn query<'a>(pairs: &[(&'a str, Option<String>)]) -> Vec<(&'a str, String)> {
    pairs
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| (*k, v.clone())))
        .collect()
}

/// Serialise a value, surfacing the JSON error type.
pub fn to_value<T: Serialize>(value: &T) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(value)?)
}
