use serde_json::Value;

use super::WebdavStorage;

#[test]
fn a_missing_url_names_the_backend() {
    let error = match WebdavStorage::new(&Value::Null) {
        Ok(_) => panic!("a source without a url must be rejected"),
        Err(e) => e,
    };
    assert!(
        error.to_string().contains("webdav：必须提供 url"),
        "got: {error}"
    );
}

#[test]
fn a_malformed_url_is_a_configuration_error() {
    let opts = serde_json::json!({ "url": "not a url" });
    let error = match WebdavStorage::new(&opts) {
        Ok(_) => panic!("a malformed url must be rejected at construction"),
        Err(e) => e,
    };
    assert!(
        error.to_string().contains("不是合法的绝对地址"),
        "got: {error}"
    );
}

#[test]
fn hash_key_uses_slash() {
    assert_eq!(crate::util::hash_to_filename("abcdef"), "ab/abcdef");
}
