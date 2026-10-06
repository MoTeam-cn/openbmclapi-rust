use serde_json::{json, Value};

use super::parse;

fn parsed(input: &str) -> Value {
    parse(input).expect("the document should parse")
}

#[test]
fn nested_mappings_build_nested_objects() {
    let value = parsed("outer:\n  inner:\n    leaf: 1\n");
    assert_eq!(value, json!({"outer": {"inner": {"leaf": 1}}}));
}

#[test]
fn a_root_sequence_keeps_its_order() {
    let value = parsed("- first\n- second\n");
    assert_eq!(value, json!(["first", "second"]));
}

#[test]
fn a_sequence_of_mappings_builds_objects() {
    let value =
        parsed("sources:\n  - type: alist\n    options:\n      url: https://a\n  - type: webdav\n");
    assert_eq!(
        value,
        json!({"sources": [
            {"type": "alist", "options": {"url": "https://a"}},
            {"type": "webdav"}
        ]})
    );
}

#[test]
fn comments_are_ignored_outside_quotes() {
    let value = parsed("# leading\na: 1 # trailing\n# middle\nb: \"x # not a comment\"\n");
    assert_eq!(value, json!({"a": 1, "b": "x # not a comment"}));
}

#[test]
fn a_hash_without_a_preceding_space_stays_in_the_scalar() {
    let value = parsed("url: https://example.com/page#fragment\n");
    assert_eq!(value, json!({"url": "https://example.com/page#fragment"}));
}

#[test]
fn a_key_with_nothing_after_the_colon_opens_an_empty_mapping() {
    let value = parsed("a:\nb: 1\n");
    assert_eq!(value, json!({"a": {}, "b": 1}));
}

#[test]
fn a_key_with_nothing_after_the_colon_still_takes_a_nested_block() {
    let value = parsed("a:\n  b: 1\n");
    assert_eq!(value, json!({"a": {"b": 1}}));
}

#[test]
fn explicit_null_spellings_stay_null() {
    let value = parsed("a: null\nb: ~\nc: Null\nd: \"\"\n");
    assert_eq!(value, json!({"a": null, "b": null, "c": null, "d": ""}));
}

#[test]
fn anchors_are_rejected_with_their_line() {
    let error = parse("a: 1\nb: &anchor value\n").expect_err("anchors are unsupported");
    assert_eq!(error.line, 2);
    assert!(error.message.contains("anchor"), "{}", error.message);
}

#[test]
fn aliases_are_rejected_with_their_line() {
    let error = parse("a: 1\nb: *anchor\n").expect_err("aliases are unsupported");
    assert_eq!(error.line, 2);
    assert!(error.message.contains("aliases"), "{}", error.message);
}

#[test]
fn tags_are_rejected_with_their_line() {
    let error = parse("a: 1\nb: !tag value\n").expect_err("tags are unsupported");
    assert_eq!(error.line, 2);
    assert!(error.message.contains("tags"), "{}", error.message);
}

#[test]
fn multi_line_scalars_are_rejected_with_their_line() {
    let error = parse("a: 1\nb: |\n  text\n").expect_err("block scalars are unsupported");
    assert_eq!(error.line, 2);
    assert!(error.message.contains("multi-line"), "{}", error.message);
}

#[test]
fn document_markers_are_rejected_with_their_line() {
    let error = parse("a: 1\n---\nb: 2\n").expect_err("document markers are unsupported");
    assert_eq!(error.line, 2);
    assert!(
        error.message.contains("document markers"),
        "{}",
        error.message
    );
}

#[test]
fn merge_keys_are_rejected_with_their_line() {
    let error = parse("a: 1\n<<: *base\n").expect_err("merge keys are unsupported");
    assert_eq!(error.line, 2);
    assert!(error.message.contains("merge keys"), "{}", error.message);
}

#[test]
fn duplicate_keys_are_rejected_with_their_line() {
    let error = parse("a: 1\nb: 2\na: 3\n").expect_err("duplicate keys are unsupported");
    assert_eq!(error.line, 3);
    assert!(error.message.contains("duplicate key"), "{}", error.message);
}
