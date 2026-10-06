//! AWS Signature Version 4 signing and presigned-URL construction.

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use super::endpoint::Endpoint;
use crate::storage::shared::encode_component;

/// Lowercase hex SHA-256 digest.
pub(super) fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// HMAC-SHA256 over `data` with `key`.
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// AWS SigV4 canonical query string: encoded, then sorted.
pub(super) fn canonical_query_string(query: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(key, value)| (encode_component(key), encode_component(value)))
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// AWS SigV4 canonical headers plus the matching `SignedHeaders` list.
fn canonical_headers(headers: &[(String, String)]) -> (String, String) {
    let mut sorted: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    sorted.sort();
    let block: String = sorted
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect();
    let signed = sorted
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>()
        .join(";");
    (block, signed)
}

/// Derive the SigV4 signing key for `date`/`region`/`service`.
fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// Sign a request with AWS Signature Version 4 and return the headers to send.
pub(super) fn sign_v4(
    endpoint: &Endpoint,
    method: &str,
    canonical_uri: &str,
    query: &[(String, String)],
    payload_hash: &str,
    now: DateTime<Utc>,
) -> Vec<(String, String)> {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let scope = format!("{date}/{}/s3/aws4_request", endpoint.region);
    let headers = vec![
        ("host".to_string(), endpoint.authority()),
        ("x-amz-content-sha256".to_string(), payload_hash.to_string()),
        ("x-amz-date".to_string(), amz_date.clone()),
    ];
    let (header_block, signed_headers) = canonical_headers(&headers);
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{}\n{header_block}\n{signed_headers}\n{payload_hash}",
        canonical_query_string(query),
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes()),
    );
    let signature = hex::encode(hmac_sha256(
        &signing_key(&endpoint.secret_key, &date, &endpoint.region, "s3"),
        string_to_sign.as_bytes(),
    ));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        endpoint.access_key,
    );
    vec![
        ("host".to_string(), endpoint.authority()),
        ("x-amz-content-sha256".to_string(), payload_hash.to_string()),
        ("x-amz-date".to_string(), amz_date),
        ("authorization".to_string(), authorization),
    ]
}

/// Build a presigned `GET` URL with the given expiry in seconds.
pub(super) fn presign_get(
    endpoint: &Endpoint,
    canonical_uri: &str,
    expires: u64,
    disposition: Option<&str>,
) -> String {
    let now = Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let scope = format!("{date}/{}/s3/aws4_request", endpoint.region);
    let mut query = vec![
        (
            "X-Amz-Algorithm".to_string(),
            "AWS4-HMAC-SHA256".to_string(),
        ),
        (
            "X-Amz-Credential".to_string(),
            format!("{}/{}", endpoint.access_key, scope),
        ),
        ("X-Amz-Date".to_string(), amz_date.clone()),
        ("X-Amz-Expires".to_string(), expires.to_string()),
        ("X-Amz-SignedHeaders".to_string(), "host".to_string()),
    ];
    if let Some(disposition) = disposition {
        query.push((
            "response-content-disposition".to_string(),
            disposition.to_string(),
        ));
    }
    let canonical_query = canonical_query_string(&query);
    let header_block = format!("host:{}\n", endpoint.authority());
    let canonical_request =
        format!("GET\n{canonical_uri}\n{canonical_query}\n{header_block}\nhost\nUNSIGNED-PAYLOAD");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes()),
    );
    let signature = hex::encode(hmac_sha256(
        &signing_key(&endpoint.secret_key, &date, &endpoint.region, "s3"),
        string_to_sign.as_bytes(),
    ));
    format!(
        "{}{}?{canonical_query}&X-Amz-Signature={signature}",
        endpoint.base_url(),
        canonical_uri,
    )
}

#[cfg(test)]
#[path = "sigv4_test.rs"]
mod tests;
