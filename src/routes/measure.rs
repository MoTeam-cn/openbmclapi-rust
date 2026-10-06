//! `GET /measure/:size` — bandwidth probe used by the master.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use tracing::debug;

use crate::cluster::Cluster;
use crate::storage::measure;
use crate::util::check_sign;

/// One megabyte of incompressible bytes.
///
/// A repeating pattern compresses away in transit, so a proxy or CDN applying
/// gzip would report a throughput unrelated to moving real data. The stored
/// probes come from the same generator, so both paths measure the same thing.
fn probe_chunk() -> Vec<u8> {
    measure::payload(1)
}

/// Stream `size` megabytes back to the caller.
///
/// A stored probe is preferred: it travels the real path from the backend to the
/// client, which is what the master is trying to measure. Whether that happens at
/// all is the operator's call — `measure_redirect` switches it off globally and
/// each source can opt out on its own — and any size without a stored object falls
/// back to generating one in process, so the endpoint never regresses to an error.
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

    if cluster.config.measure_redirect && cluster.config.measure_sizes.contains(&(count as u64)) {
        match cluster.storage.serve_measure(count as u64).await {
            Ok(Some((response, stat))) => {
                cluster.record_served(stat).await;
                return response;
            }
            Ok(None) => debug!(size = count, "没有已存的测速对象，现场生成"),
            Err(e) => debug!(size = count, error = %e, "已存的测速对象不可用，现场生成"),
        }
    }

    let chunk = probe_chunk();
    let stream = futures::stream::iter(
        (0..count).map(move |_| Ok::<_, std::io::Error>(bytes::Bytes::from(chunk.clone()))),
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_LENGTH, (count * 1024 * 1024).to_string())
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
