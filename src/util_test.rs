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
