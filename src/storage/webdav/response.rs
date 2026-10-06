//! HTTP response helpers shared with the AList backend.

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;

/// 302 redirect response.
pub(crate) fn redirect(url: String) -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, url)
        .body(Body::empty())
        .expect("valid redirect response")
}

/// Empty 200 response.
pub(crate) fn empty_ok() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())
        .expect("valid empty response")
}
