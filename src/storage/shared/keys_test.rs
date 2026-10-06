use super::{encode_component, encode_filename, encode_path, join_object_key, strip_prefix_key};

#[test]
fn join_key_variants() {
    assert_eq!(join_object_key("", ".check"), ".check");
    assert_eq!(join_object_key("pre", ".check"), "pre/.check");
    assert_eq!(join_object_key("pre/", "/ab/hash"), "pre/ab/hash");
    assert_eq!(join_object_key("pre", "ab/hash"), "pre/ab/hash");
}

#[test]
fn strip_prefix_variants() {
    assert_eq!(strip_prefix_key("pre/ab/hash", "pre"), "ab/hash");
    assert_eq!(strip_prefix_key("ab/hash", ""), "ab/hash");
    assert_eq!(strip_prefix_key("other/ab/hash", "pre"), "other/ab/hash");
}

#[test]
fn encoding_rules() {
    assert_eq!(encode_path("ab/cd ef"), "ab/cd%20ef");
    assert_eq!(encode_path("ab/c d"), "ab/c%20d");
    assert_eq!(encode_component("a/b"), "a%2Fb");
    assert_eq!(encode_component("ab/c"), "ab%2Fc");
    assert_eq!(encode_filename("a b\"c"), "a%20b%22c");
    assert_eq!(encode_filename("keep-_.!~*'()"), "keep-_.!~*'()");
}
