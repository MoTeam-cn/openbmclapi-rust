//! Aliyun OSS Signature V1: header signing and presigned URLs.

use base64::Engine as _;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha1::Sha1;

use super::storage::OssStorage;

/// Base64 HMAC-SHA1 over `data` with `secret`, as OSS V1 requires.
pub(super) fn hmac_sha1_base64(secret: &str, data: &str) -> String {
    let mut mac =
        Hmac::<Sha1>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(data.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// OSS V1 `CanonicalizedResource`: bucket, key and sorted sub-resources.
pub(super) fn canonicalized_resource(
    bucket: &str,
    key: &str,
    subresources: &[(String, String)],
) -> String {
    let mut resource = format!("/{bucket}/{key}");
    let mut parts: Vec<String> = subresources
        .iter()
        .map(|(name, value)| {
            if value.is_empty() {
                name.clone()
            } else {
                format!("{name}={value}")
            }
        })
        .collect();
    parts.sort();
    if !parts.is_empty() {
        resource.push('?');
        resource.push_str(&parts.join("&"));
    }
    resource
}

/// OSS V1 `StringToSign` for a header-signed request.
///
/// Only used for requests that set neither Content-MD5 nor Content-Type and no
/// `x-oss-*` headers.
fn string_to_sign_v1(method: &str, date: &str, resource: &str) -> String {
    format!("{method}\n\n\n{date}\n{resource}")
}

/// OSS V1 `StringToSign` for a URL-signed (presigned) GET.
pub(super) fn string_to_sign_presign(expires: i64, resource: &str) -> String {
    format!("GET\n\n\n{expires}\n{resource}")
}

/// RFC 1123 date in GMT, the format OSS V1 expects.
pub(super) fn http_date() -> String {
    Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

impl OssStorage {
    /// Build the OSS V1 `Authorization` header for a request.
    pub(super) fn authorization(
        &self,
        method: &str,
        key: &str,
        subresources: &[(String, String)],
        date: &str,
    ) -> String {
        let resource = canonicalized_resource(&self.config.bucket, key, subresources);
        let string_to_sign = string_to_sign_v1(method, date, &resource);
        let signature = hmac_sha1_base64(&self.config.access_key_secret, &string_to_sign);
        format!("OSS {}:{signature}", self.config.access_key_id)
    }
}

#[cfg(test)]
#[path = "signature_test.rs"]
mod tests;
