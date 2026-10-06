use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use sha1::{Digest, Sha1};

/// Current unix time in milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Map a content hash onto its two-level storage key, e.g.
/// `abcdef...` -> `ab/abcdef...`.
///
/// Always uses `/` as separator so that remote object keys are identical on
/// every platform. Local file storage splits this on `/` and joins with the
/// native separator.
pub fn hash_to_filename(hash: &str) -> String {
    let prefix_len = 2.min(hash.len());
    format!("{}/{}", &hash[..prefix_len], hash)
}

/// Verify the `s`/`e` signature query parameters.
///
/// `sign = base64url(sha1(secret || hash || e))` and `e` is a base36 unix
/// millisecond deadline.
pub fn check_sign(hash: &str, secret: &str, query: &HashMap<String, String>) -> bool {
    let (Some(sign), Some(expires)) = (query.get("s"), query.get("e")) else {
        return false;
    };
    let mut hasher = Sha1::new();
    hasher.update(secret.as_bytes());
    hasher.update(hash.as_bytes());
    hasher.update(expires.as_bytes());
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());
    if expected != *sign {
        return false;
    }
    match i64::from_str_radix(expires, 36) {
        Ok(deadline) => now_ms() < deadline,
        Err(_) => false,
    }
}

/// Number of bytes a ranged response will transfer.
///
/// Mirrors the Node `getSize` helper: an unparsable or unsatisfiable range
/// falls back to the full object size.
pub fn get_size(size: i64, range: Option<&str>) -> i64 {
    let Some(range) = range else { return size };
    match parse_ranges(size, range) {
        Some(ranges) => ranges.iter().map(|(start, end)| end - start + 1).sum(),
        None => size,
    }
}

/// Minimal `range-parser` equivalent returning inclusive `(start, end)` pairs.
///
/// Returns `None` when the header is invalid or fully unsatisfiable, which is
/// what makes `get_size` fall back to the full size.
fn parse_ranges(size: i64, header: &str) -> Option<Vec<(i64, i64)>> {
    let spec = header.strip_prefix("bytes=")?;
    if size <= 0 {
        return None;
    }
    let mut out = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (raw_start, raw_end) = part.split_once('-')?;
        let range = if raw_start.is_empty() {
            // Suffix range: last N bytes.
            let suffix: i64 = raw_end.parse().ok()?;
            if suffix == 0 {
                continue;
            }
            let start = (size - suffix).max(0);
            (start, size - 1)
        } else {
            let start: i64 = raw_start.parse().ok()?;
            if start >= size {
                continue;
            }
            let end = if raw_end.is_empty() {
                size - 1
            } else {
                let end: i64 = raw_end.parse().ok()?;
                end.min(size - 1)
            };
            if end < start {
                continue;
            }
            (start, end)
        };
        out.push(range);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Parse a boolean environment value the way `env-var` does.
pub fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_to_filename_splits_two_chars() {
        assert_eq!(hash_to_filename("abcdef"), "ab/abcdef");
        assert_eq!(hash_to_filename("a"), "a/a");
    }

    #[test]
    fn sign_roundtrip() {
        let secret = "secret";
        let hash = "abcdef";
        let deadline = now_ms() + 60_000;
        let expires = base36(deadline);
        let mut hasher = Sha1::new();
        hasher.update(secret.as_bytes());
        hasher.update(hash.as_bytes());
        hasher.update(expires.as_bytes());
        let sign = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());
        let query = HashMap::from([("s".to_string(), sign), ("e".to_string(), expires)]);
        assert!(check_sign(hash, secret, &query));
        assert!(!check_sign("other", secret, &query));
    }

    fn base36(mut n: i64) -> String {
        const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        if n == 0 {
            return "0".into();
        }
        let mut buf = Vec::new();
        while n > 0 {
            buf.push(DIGITS[(n % 36) as usize]);
            n /= 36;
        }
        buf.reverse();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn size_without_range() {
        assert_eq!(get_size(100, None), 100);
    }

    #[test]
    fn size_with_ranges() {
        assert_eq!(get_size(100, Some("bytes=0-9")), 10);
        assert_eq!(get_size(100, Some("bytes=0-9,20-29")), 20);
        assert_eq!(get_size(100, Some("bytes=-10")), 10);
        assert_eq!(get_size(100, Some("bytes=50-")), 50);
        // Unsatisfiable -> full size.
        assert_eq!(get_size(100, Some("bytes=200-300")), 100);
        assert_eq!(get_size(100, Some("garbage")), 100);
    }
}
