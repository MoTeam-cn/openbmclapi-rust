//! Cluster token management.
//!
//! The agent authenticates against the master with an HMAC-SHA256 challenge
//! response and keeps the resulting bearer token fresh in the background.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use tokio::sync::RwLock;
use tracing::{debug, error, trace};

use crate::error::{Error, Result};

type HmacSha256 = Hmac<Sha256>;

/// Refresh ten minutes before expiry, matching the Node agent.
const REFRESH_MARGIN_MS: i64 = 10 * 60 * 1000;

#[derive(Debug, Deserialize)]
struct ChallengeResponse {
    challenge: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    token: String,
    ttl: i64,
}

/// Holds the cluster token and refreshes it before it expires.
#[derive(Clone)]
pub struct TokenManager {
    inner: Arc<Inner>,
}

struct Inner {
    cluster_id: String,
    cluster_secret: String,
    base: String,
    http: reqwest::Client,
    token: RwLock<Option<String>>,
    ttl_ms: AtomicI64,
    refresh_started: AtomicBool,
}

impl TokenManager {
    /// Create a manager for the given cluster credentials and master base URL.
    pub fn new(
        cluster_id: impl Into<String>,
        cluster_secret: impl Into<String>,
        version: &str,
        base: impl Into<String>,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("openbmclapi-cluster/{version}"))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .build()?;
        Ok(TokenManager {
            inner: Arc::new(Inner {
                cluster_id: cluster_id.into(),
                cluster_secret: cluster_secret.into(),
                base: base.into(),
                http,
                token: RwLock::new(None),
                ttl_ms: AtomicI64::new(0),
                refresh_started: AtomicBool::new(false),
            }),
        })
    }

    /// Return a valid token, fetching one on first use.
    pub async fn get_token(&self) -> Result<String> {
        if let Some(token) = self.inner.token.read().await.clone() {
            return Ok(token);
        }
        // Serialise concurrent first-use fetches.
        let mut guard = self.inner.token.write().await;
        if let Some(token) = guard.clone() {
            return Ok(token);
        }
        let token = self.inner.fetch_token().await?;
        *guard = Some(token.clone());
        drop(guard);
        self.inner.clone().spawn_refresh_loop();
        Ok(token)
    }

    /// Drop the cached token so the next call re-authenticates.
    pub async fn invalidate(&self) {
        *self.inner.token.write().await = None;
    }
}

impl Inner {
    fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.base.trim_end_matches('/'), path)
    }

    async fn fetch_token(&self) -> Result<String> {
        let challenge: ChallengeResponse = self
            .http
            .get(self.endpoint("openbmclapi-agent/challenge"))
            .query(&[("clusterId", self.cluster_id.as_str())])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let mut mac = HmacSha256::new_from_slice(self.cluster_secret.as_bytes())
            .map_err(|e| Error::Other(format!("invalid cluster secret: {e}")))?;
        mac.update(challenge.challenge.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let token: TokenResponse = self
            .http
            .post(self.endpoint("openbmclapi-agent/token"))
            .json(&serde_json::json!({
                "clusterId": self.cluster_id,
                "challenge": challenge.challenge,
                "signature": signature,
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        self.ttl_ms.store(token.ttl, Ordering::Relaxed);
        trace!(ttl = token.ttl, "acquired cluster token");
        Ok(token.token)
    }

    async fn refresh(&self) -> Result<()> {
        let current = self.token.read().await.clone();
        let mut body = serde_json::json!({ "clusterId": self.cluster_id });
        if let Some(token) = current {
            body["token"] = serde_json::Value::String(token);
        }
        let token: TokenResponse = self
            .http
            .post(self.endpoint("openbmclapi-agent/token"))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        self.ttl_ms.store(token.ttl, Ordering::Relaxed);
        *self.token.write().await = Some(token.token);
        debug!("refreshed cluster token");
        Ok(())
    }

    fn spawn_refresh_loop(self: Arc<Self>) {
        if self.refresh_started.swap(true, Ordering::SeqCst) {
            return;
        }
        tokio::spawn(async move {
            loop {
                let ttl = self.ttl_ms.load(Ordering::Relaxed);
                let next = if ttl > 0 {
                    (ttl - REFRESH_MARGIN_MS).max(ttl / 2).max(1_000)
                } else {
                    60_000
                };
                trace!(next_ms = next, "scheduled token refresh");
                tokio::time::sleep(Duration::from_millis(next as u64)).await;
                if let Err(e) = self.refresh().await {
                    error!(error = %e, "refresh token error");
                }
            }
        });
    }
}
