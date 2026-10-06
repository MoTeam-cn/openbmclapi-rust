use super::{split_bucket_prefix, Endpoint};

#[test]
fn endpoint_parsing() {
    let endpoint = Endpoint::parse(
        "https://key:secret@minio.example.com:9000/bucket/prefix?region=us-west-1",
        None,
    )
    .unwrap();
    assert_eq!(endpoint.scheme, "https");
    assert_eq!(endpoint.host, "minio.example.com");
    assert_eq!(endpoint.port, Some(9000));
    assert_eq!(endpoint.access_key, "key");
    assert_eq!(endpoint.secret_key, "secret");
    assert_eq!(endpoint.region, "us-west-1");
    assert_eq!(endpoint.authority(), "minio.example.com:9000");
    assert_eq!(endpoint.base_url(), "https://minio.example.com:9000");

    let (bucket, prefix) =
        split_bucket_prefix("https://key:secret@minio.example.com:9000/bucket/a/b").unwrap();
    assert_eq!(bucket, "bucket");
    assert_eq!(prefix, "a/b");
}

#[test]
fn endpoint_default_region_and_port() {
    let endpoint = Endpoint::parse("http://127.0.0.1/bucket", None).unwrap();
    assert_eq!(endpoint.region, "us-east-1");
    assert_eq!(endpoint.port, None);
    assert_eq!(endpoint.authority(), "127.0.0.1");
}
