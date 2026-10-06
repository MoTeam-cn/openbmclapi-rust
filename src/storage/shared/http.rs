//! Shared error handling and response plumbing for object-storage HTTP responses.

use crate::error::{Error, Result};

/// Turn a non-2xx response into a storage error, keeping a short body excerpt.
///
/// `backend` names the caller (`S3` / `OSS`) so failures stay identifiable.
pub(crate) async fn ensure_success(
    response: reqwest::Response,
    backend: &str,
    op: &str,
    key: &str,
) -> Result<()> {
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
        "{backend} {op} {key} failed: {status} {body}"
    )))
}

/// Object-metadata headers worth forwarding when the agent proxies a body.
const PASSTHROUGH_HEADERS: [axum::http::HeaderName; 6] = [
    axum::http::header::CONTENT_TYPE,
    axum::http::header::CONTENT_LENGTH,
    axum::http::header::CONTENT_RANGE,
    axum::http::header::ACCEPT_RANGES,
    axum::http::header::LAST_MODIFIED,
    axum::http::header::ETAG,
];

/// Copy the object-metadata headers from an upstream response onto a builder.
///
/// Anything outside the allowlist (cookies, transfer-encoding, the upstream's
/// own credentials) is deliberately dropped rather than relayed to the client.
pub(crate) fn copy_passthrough(
    upstream: &reqwest::Response,
    mut builder: axum::http::response::Builder,
) -> axum::http::response::Builder {
    for name in PASSTHROUGH_HEADERS {
        if let Some(value) = upstream.headers().get(&name) {
            builder = builder.header(name, value.clone());
        }
    }
    builder
}
