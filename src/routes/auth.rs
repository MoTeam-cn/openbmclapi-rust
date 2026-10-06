//! `GET /auth` — nginx `auth_request` endpoint.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::cluster::Cluster;
use crate::util::check_sign;

/// Validate the `x-original-uri` signature and answer `204`.
pub async fn auth(
    State(cluster): State<Arc<Cluster>>,
    headers: HeaderMap,
    RawQuery(_query): RawQuery,
) -> Response {
    let Some(original) = headers.get("x-original-uri").and_then(|v| v.to_str().ok()) else {
        return (StatusCode::FORBIDDEN, "invalid sign").into_response();
    };
    let Ok(url) = url::Url::parse(original)
        .or_else(|_| url::Url::parse(&format!("http://localhost{original}")))
    else {
        return (StatusCode::FORBIDDEN, "invalid sign").into_response();
    };
    let hash = url
        .path()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let params: HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    if !check_sign(&hash, &cluster.config.cluster_secret, &params) && !cluster.config.disable_sign {
        return (StatusCode::FORBIDDEN, "invalid sign").into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}
