//! S3 HTTP operations: PUT, HEAD, DELETE and `ListObjectsV2`.
//!
//! Each call is SigV4-signed against either the internal or the public
//! endpoint; the signing primitives live in [`super::sigv4`].

use axum::http::StatusCode;
use bytes::Bytes;
use chrono::Utc;

use crate::error::{Error, Result};
use crate::storage::shared::{ensure_success, ListedObject};

use super::endpoint::Endpoint;
use super::sigv4::{canonical_query_string, sha256_hex, sign_v4};
use super::storage::MinioStorage;
use super::xml::parse_s3_list;

impl MinioStorage {
    /// Send one SigV4-signed request.
    pub(super) async fn send_signed(
        &self,
        endpoint: &Endpoint,
        method: reqwest::Method,
        uri: &str,
        query: &[(String, String)],
        body: Option<Bytes>,
    ) -> Result<reqwest::Response> {
        let payload_hash = match &body {
            Some(bytes) => sha256_hex(bytes),
            None => sha256_hex(b""),
        };
        let headers = sign_v4(
            endpoint,
            method.as_str(),
            uri,
            query,
            &payload_hash,
            Utc::now(),
        );
        let url = if query.is_empty() {
            format!("{}{}", endpoint.base_url(), uri)
        } else {
            format!(
                "{}{}?{}",
                endpoint.base_url(),
                uri,
                canonical_query_string(query)
            )
        };
        let mut request = self.http.request(method, url);
        for (name, value) in &headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(bytes) = body {
            request = request.body(bytes);
        }
        Ok(request.send().await?)
    }

    /// Upload an object through the given endpoint.
    pub(super) async fn put_object(
        &self,
        endpoint: &Endpoint,
        key: &str,
        body: Bytes,
    ) -> Result<()> {
        let uri = self.object_uri(key);
        let response = self
            .send_signed(endpoint, reqwest::Method::PUT, &uri, &[], Some(body))
            .await?;
        ensure_success(response, "S3", "PUT", key).await
    }

    /// HEAD an object; `false` only for a definitive 404.
    pub(super) async fn head_object(&self, endpoint: &Endpoint, key: &str) -> Result<bool> {
        let uri = self.object_uri(key);
        let response = self
            .send_signed(endpoint, reqwest::Method::HEAD, &uri, &[], None)
            .await?;
        if response.status().is_success() {
            Ok(true)
        } else if response.status() == StatusCode::NOT_FOUND {
            Ok(false)
        } else {
            Err(Error::storage(format!(
                "S3 HEAD {key} failed: {}",
                response.status()
            )))
        }
    }

    /// DELETE an object, treating 404 as success.
    pub(super) async fn delete_object(&self, endpoint: &Endpoint, key: &str) -> Result<()> {
        let uri = self.object_uri(key);
        let response = self
            .send_signed(endpoint, reqwest::Method::DELETE, &uri, &[], None)
            .await?;
        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(Error::storage(format!(
            "S3 DELETE {key} failed: {status} {body}"
        )))
    }

    /// List every object under the configured prefix (recursive).
    pub(super) async fn list_all(&self, endpoint: &Endpoint) -> Result<Vec<ListedObject>> {
        let mut objects = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query: Vec<(String, String)> = vec![
                ("list-type".to_string(), "2".to_string()),
                ("max-keys".to_string(), "1000".to_string()),
            ];
            if !self.prefix.is_empty() {
                query.push(("prefix".to_string(), self.prefix.clone()));
            }
            if let Some(token) = &token {
                query.push(("continuation-token".to_string(), token.clone()));
            }
            let response = self
                .send_signed(
                    endpoint,
                    reqwest::Method::GET,
                    &self.bucket_uri(),
                    &query,
                    None,
                )
                .await?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(Error::storage(format!(
                    "S3 ListObjectsV2 failed: {status} {body}"
                )));
            }
            let body = response.text().await?;
            let page = parse_s3_list(&body);
            objects.extend(page.objects);
            if !page.is_truncated {
                break;
            }
            match page.next_token {
                Some(next) if !next.is_empty() => token = Some(next),
                _ => break,
            }
        }
        Ok(objects)
    }
}
