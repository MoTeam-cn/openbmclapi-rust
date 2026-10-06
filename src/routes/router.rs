//! Router assembly and access logging.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{header, HeaderName, Version};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use tracing::info;

use crate::cluster::Cluster;
use crate::server::PeerAddr;

use super::{auth, download, measure};

/// Build the router shared by the HTTP/1.1 and HTTP/2 listeners.
pub fn router(cluster: Arc<Cluster>) -> Router {
    let mut router = Router::new()
        .route("/auth", get(auth::auth))
        .route("/download/{hash}", get(download::download))
        .route("/measure/{size}", get(measure::measure));

    if !cluster.config.disable_access_log {
        router = router.layer(middleware::from_fn(log_request));
    }

    router.with_state(cluster)
}

/// One access-log line per request, unless `DISABLE_ACCESS_LOG` is set.
///
/// The fields are the ones morgan's `combined` format prints; the line itself
/// is assembled by the access formatter, which is what keeps the query string
/// and the client version in the record.
async fn log_request(request: Request, next: Next) -> Response {
    let remote = request
        .extensions()
        .get::<PeerAddr>()
        .map(|peer| peer.0.ip().to_string())
        .unwrap_or_else(|| "-".to_string());
    let method = request.method().clone();
    let version = http_version(request.version());
    let uri = request
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_default();
    let referer = header_value(&request, header::REFERER);
    let user_agent = header_value(&request, header::USER_AGENT);

    let response = next.run(request).await;

    let length = response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("-")
        .to_string();
    info!(
        target: crate::logger::ACCESS_TARGET,
        remote = %remote,
        method = %method,
        uri = %uri,
        version = %version,
        status = response.status().as_u16(),
        length = %length,
        referer = %referer,
        user_agent = %user_agent,
        "request"
    );
    response
}

/// The version string morgan prints, for example `1.1` or `2.0`.
fn http_version(version: Version) -> &'static str {
    match version {
        Version::HTTP_09 => "0.9",
        Version::HTTP_10 => "1.0",
        Version::HTTP_11 => "1.1",
        Version::HTTP_2 => "2.0",
        Version::HTTP_3 => "3.0",
        _ => "-",
    }
}

/// A header value, or `-` when the client did not send it.
fn header_value(request: &Request, name: HeaderName) -> String {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("-")
        .to_string()
}
