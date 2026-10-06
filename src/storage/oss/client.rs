//! Object-level OSS HTTP operations: PUT, HEAD, DELETE and ListObjects.

use axum::http::{header, StatusCode};
use bytes::Bytes;

use crate::error::{Error, Result};
use crate::storage::shared::{encode_component, ensure_success, ListedObject};

use super::signature::http_date;
use super::storage::OssStorage;
use super::xml::parse_oss_list;

/// The largest page OSS returns from one listing call.
const MAX_KEYS: &str = "1000";

impl OssStorage {
    /// Upload an object.
    pub(super) async fn put_object(&self, key: &str, body: Bytes) -> Result<()> {
        let date = http_date();
        let auth = self.authorization("PUT", key, &[], &date);
        let response = self
            .http
            .put(self.object_url(key))
            .header(header::DATE, date.as_str())
            .header(header::AUTHORIZATION, auth.as_str())
            .body(body)
            .send()
            .await?;
        ensure_success(response, "OSS", "PUT", key).await
    }

    /// HEAD an object; `false` only for a definitive 404.
    pub(super) async fn head_object(&self, key: &str) -> Result<bool> {
        let date = http_date();
        let auth = self.authorization("HEAD", key, &[], &date);
        let response = self
            .http
            .head(self.object_url(key))
            .header(header::DATE, date.as_str())
            .header(header::AUTHORIZATION, auth.as_str())
            .send()
            .await?;
        if response.status().is_success() {
            Ok(true)
        } else if response.status() == StatusCode::NOT_FOUND {
            Ok(false)
        } else {
            Err(Error::storage(format!(
                "OSS HEAD {key} failed: {}",
                response.status()
            )))
        }
    }

    /// DELETE an object, treating 404 as success.
    pub(super) async fn delete_object(&self, key: &str) -> Result<()> {
        let date = http_date();
        let auth = self.authorization("DELETE", key, &[], &date);
        let response = self
            .http
            .delete(self.object_url(key))
            .header(header::DATE, date.as_str())
            .header(header::AUTHORIZATION, auth.as_str())
            .send()
            .await?;
        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(Error::storage(format!(
            "OSS DELETE {key} failed: {status} {body}"
        )))
    }

    /// List every object under the configured prefix (recursive, marker paged).
    pub(super) async fn list_all(&self) -> Result<Vec<ListedObject>> {
        let mut objects = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut query: Vec<(String, String)> =
                vec![("max-keys".to_string(), MAX_KEYS.to_string())];
            if !self.config.prefix.is_empty() {
                query.push(("prefix".to_string(), self.config.prefix.clone()));
            }
            if let Some(marker) = &marker {
                query.push(("marker".to_string(), marker.clone()));
            }
            let date = http_date();
            let auth = self.authorization("GET", "", &query, &date);
            let query_string = query
                .iter()
                .map(|(key, value)| {
                    format!("{}={}", encode_component(key), encode_component(value))
                })
                .collect::<Vec<_>>()
                .join("&");
            let url = format!("{}/?{}", self.base, query_string);
            let response = self
                .http
                .get(url)
                .header(header::DATE, date.as_str())
                .header(header::AUTHORIZATION, auth.as_str())
                .send()
                .await?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(Error::storage(format!(
                    "OSS ListObjects failed: {status} {body}"
                )));
            }
            let body = response.text().await?;
            let page = parse_oss_list(&body);
            let last_key = page.objects.last().map(|object| object.key.clone());
            objects.extend(page.objects);
            if !page.is_truncated {
                break;
            }
            marker = page
                .next_marker
                .filter(|value| !value.is_empty())
                .or(last_key);
            if marker.is_none() {
                break;
            }
        }
        Ok(objects)
    }
}
