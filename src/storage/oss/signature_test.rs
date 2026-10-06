use super::*;

#[test]
fn canonicalized_resource_sorts_subresources() {
    let subresources = vec![
        ("prefix".to_string(), "ab".to_string()),
        ("max-keys".to_string(), "1000".to_string()),
        ("marker".to_string(), "x".to_string()),
    ];
    assert_eq!(
        canonicalized_resource("bucket", "", &subresources),
        "/bucket/?marker=x&max-keys=1000&prefix=ab"
    );
    assert_eq!(
        canonicalized_resource("bucket", "ab/hash", &[]),
        "/bucket/ab/hash"
    );
}

#[test]
fn string_to_sign_layout() {
    let resource = canonicalized_resource("bucket", "ab/hash", &[]);
    assert_eq!(
        string_to_sign_v1("PUT", "Wed, 01 Jan 2020 00:00:00 GMT", &resource),
        "PUT\n\n\nWed, 01 Jan 2020 00:00:00 GMT\n/bucket/ab/hash"
    );
    assert_eq!(
        string_to_sign_presign(1234567890, &resource),
        "GET\n\n\n1234567890\n/bucket/ab/hash"
    );
}
