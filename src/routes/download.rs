//! `GET /download/:hash` — the hot path of the agent.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use tracing::debug;

use crate::cluster::Cluster;
use crate::error::Error;
use crate::storage::ServeRequest;
use crate::util::{check_sign, hash_to_filename};

/// Serve a cached object, downloading it from the master on a miss.
pub async fn download(
    State(cluster): State<Arc<Cluster>>,
    Path(hash): Path<String>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    if !hash.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return StatusCode::NOT_FOUND.into_response();
    }
    let hash = hash.to_ascii_lowercase();
    let params = parse_query(raw.as_deref());
    let sign_valid = check_sign(&hash, &cluster.config.cluster_secret, &params);
    if !sign_valid && !cluster.config.disable_sign {
        return (StatusCode::FORBIDDEN, "invalid sign").into_response();
    }

    let hash_path = hash_to_filename(&hash);
    if let Err(e) = cluster.ensure_downloaded(&hash).await {
        if matches!(e, Error::NotFound) {
            return StatusCode::NOT_FOUND.into_response();
        }
        debug!(error = %e, hash, "download from master failed");
        return StatusCode::NOT_FOUND.into_response();
    }

    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let name = params.get("name").map(|s| s.as_str());
    let request = ServeRequest {
        hash: &hash,
        hash_path: &hash_path,
        range,
        name,
    };

    match cluster.storage.serve(request).await {
        Ok((mut response, stat)) => {
            if let Ok(value) = hash.parse::<axum::http::HeaderValue>() {
                response.headers_mut().insert("x-bmclapi-hash", value);
            }
            if let Ok(value) = cluster.config.cluster_id.parse::<axum::http::HeaderValue>() {
                response.headers_mut().insert("x-bmclapi-id", value);
            }
            cluster.record_served(stat).await;
            response
        }
        Err(Error::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(Error::UpstreamUnavailable { retry_in_ms }) => unavailable(retry_in_ms),
        // A 5xx that reached us through the backend means the backend is the
        // one struggling; shed the request with a hint instead of a hard 500.
        Err(Error::Status { status, .. }) if status >= 500 => unavailable(DEFAULT_RETRY_AFTER_MS),
        Err(e) => {
            debug!(error = %e, hash, "storage serve failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Fallback `Retry-After` when the backend gives no hint of its own.
const DEFAULT_RETRY_AFTER_MS: u64 = 5_000;

/// 503 plus a `Retry-After` hint, so clients back off instead of retrying hard.
fn unavailable(retry_in_ms: u64) -> Response {
    let seconds = retry_in_ms.div_ceil(1000).max(1);
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        "storage backend temporarily unavailable",
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

fn parse_query(raw: Option<&str>) -> HashMap<String, String> {
    match raw {
        Some(raw) => form_urlencoded::parse(raw.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect(),
        None => HashMap::new(),
    }
}
