//! HTTP surface of the agent: `/auth`, `/download/:hash` and `/measure/:size`.

pub mod auth;
pub mod download;
pub mod measure;

use std::sync::Arc;

use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use tracing::info;

use crate::cluster::Cluster;

/// Build the router shared by the HTTP/1.1 and HTTP/2 listeners.
pub fn router(cluster: Arc<Cluster>) -> Router {
    let access_log = !cluster.config.disable_access_log;
    let mut router = Router::new()
        .route("/auth", get(auth::auth))
        .route("/download/{hash}", get(download::download))
        .route("/measure/{size}", get(measure::measure));

    if access_log {
        router = router.layer(middleware::from_fn(log_request));
    }

    router.with_state(cluster)
}

async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_default();
    let started = std::time::Instant::now();
    let response = next.run(request).await;
    info!(
        %method,
        path,
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "request"
    );
    response
}
