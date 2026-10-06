//! `GET /measure/:size` — bandwidth probe used by the master.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::cluster::Cluster;
use crate::util::check_sign;

/// Megabyte payload template (`0066ccff` repeated).
fn template() -> Vec<u8> {
    let mut buffer = Vec::with_capacity(1024 * 1024);
    for _ in 0..(1024 * 1024 / 4) {
        buffer.extend_from_slice(&[0x00, 0x66, 0xcc, 0xff]);
    }
    buffer
}

/// Stream `size` megabytes back to the caller.
pub async fn measure(
    State(cluster): State<Arc<Cluster>>,
    Path(size): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    if !size.chars().all(|c| c.is_ascii_digit()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let params: HashMap<String, String> = raw
        .as_deref()
        .map(|raw| {
            form_urlencoded::parse(raw.as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        })
        .unwrap_or_default();
    let signed_path = format!("/measure/{size}");
    if !check_sign(&signed_path, &cluster.config.cluster_secret, &params) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(count) = size.parse::<usize>() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if count > 200 {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let chunk = template();
    let stream = futures::stream::iter(
        (0..count).map(move |_| Ok::<_, std::io::Error>(bytes::Bytes::from(chunk.clone()))),
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_LENGTH, (count * 1024 * 1024).to_string())
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
