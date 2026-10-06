//! Shared error handling for object-storage HTTP responses.

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
