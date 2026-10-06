use serde_json::json;

use super::*;

#[test]
fn config_defaults_and_default_endpoint() {
    let config = OssConfig::parse(&json!({
        "accessKeyId": "a",
        "accessKeySecret": "b",
        "bucket": "c"
    }))
    .unwrap();
    assert!(!config.internal);
    assert_eq!(config.prefix, "");
    assert!(config.proxy);
    assert!(!config.cname);
    assert!(config.endpoint.is_none());
    assert_eq!(
        build_base_url(&config).unwrap(),
        "https://c.oss-cn-hangzhou.aliyuncs.com"
    );
}

#[test]
fn config_region_and_internal() {
    let config = OssConfig::parse(&json!({
        "accessKeyId": "a",
        "accessKeySecret": "b",
        "bucket": "c",
        "region": "cn-beijing",
        "internal": true
    }))
    .unwrap();
    assert_eq!(
        build_base_url(&config).unwrap(),
        "https://c.oss-cn-beijing-internal.aliyuncs.com"
    );
}

#[test]
fn config_endpoint_adds_scheme() {
    let config = OssConfig::parse(&json!({
        "accessKeyId": "a",
        "accessKeySecret": "b",
        "bucket": "c",
        "endpoint": "oss.example.com"
    }))
    .unwrap();
    assert_eq!(build_base_url(&config).unwrap(), "https://oss.example.com");

    let config = OssConfig::parse(&json!({
        "accessKeyId": "a",
        "accessKeySecret": "b",
        "bucket": "c",
        "endpoint": "http://oss.example.com/"
    }))
    .unwrap();
    assert_eq!(build_base_url(&config).unwrap(), "http://oss.example.com");
}

#[test]
fn config_cname_drops_bucket() {
    let config = OssConfig::parse(&json!({
        "accessKeyId": "a",
        "accessKeySecret": "b",
        "bucket": "c",
        "cname": true
    }))
    .unwrap();
    assert_eq!(
        build_base_url(&config).unwrap(),
        "https://oss-cn-hangzhou.aliyuncs.com"
    );
}

#[test]
fn config_requires_credentials() {
    let error = OssConfig::parse(&json!({ "bucket": "c" })).unwrap_err();
    assert!(matches!(error, Error::Config(_)));
}
