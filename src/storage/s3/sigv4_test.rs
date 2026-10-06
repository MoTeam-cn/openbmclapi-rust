use super::super::endpoint::Endpoint;
use super::{canonical_headers, canonical_query_string, presign_get};

#[test]
fn canonical_query_is_sorted_and_encoded() {
    let query = vec![
        ("prefix".to_string(), "a b".to_string()),
        ("list-type".to_string(), "2".to_string()),
    ];
    assert_eq!(canonical_query_string(&query), "list-type=2&prefix=a%20b");
}

#[test]
fn canonical_headers_layout() {
    let headers = vec![
        ("x-amz-date".to_string(), "20200101T000000Z".to_string()),
        ("host".to_string(), "example.com".to_string()),
    ];
    let (block, signed) = canonical_headers(&headers);
    assert_eq!(block, "host:example.com\nx-amz-date:20200101T000000Z\n");
    assert_eq!(signed, "host;x-amz-date");
}

#[test]
fn presign_shape() {
    let endpoint =
        Endpoint::parse("https://key:secret@minio.example.com:9000/bucket", None).unwrap();
    let url = presign_get(
        &endpoint,
        "/bucket/ab/hash",
        60,
        Some("attachment; filename=\"x\""),
    );
    assert!(url.starts_with("https://minio.example.com:9000/bucket/ab/hash?"));
    assert!(url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
    assert!(url.contains("X-Amz-SignedHeaders=host"));
    assert!(url.contains("X-Amz-Signature="));
    assert!(url.contains("response-content-disposition="));
}
