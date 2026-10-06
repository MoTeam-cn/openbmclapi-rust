use serde_json::{json, Value};

use crate::config::yaml::block::parse;

fn parsed(input: &str) -> Value {
    parse(input).expect("the document should parse")
}

#[test]
fn flow_collections_nest_to_any_depth() {
    let value = parsed("a: [1, 2, [3, 4]]\nb: {x: 1, y: {z: [true, false]}}\n");
    assert_eq!(
        value,
        json!({"a": [1, 2, [3, 4]], "b": {"x": 1, "y": {"z": [true, false]}}})
    );
}

#[test]
fn flow_collections_accept_quoted_scalars() {
    let value = parsed("a: [\"x, y\", 'z']\n");
    assert_eq!(value, json!({"a": ["x, y", "z"]}));
}

#[test]
fn flow_collections_may_span_lines() {
    let value = parsed("a: [\n  1,\n  2,\n]\nb: {\n  x: 1,\n  y: 2,\n}\n");
    assert_eq!(value, json!({"a": [1, 2], "b": {"x": 1, "y": 2}}));
}

#[test]
fn empty_flow_collections_become_empty_containers() {
    let value = parsed("a: []\nb: {}\n");
    assert_eq!(value, json!({"a": [], "b": {}}));
}

#[test]
fn a_missing_flow_element_becomes_null() {
    let value = parsed("a: [1, , 3]\n");
    assert_eq!(value, json!({"a": [1, null, 3]}));
}

#[test]
fn duplicate_flow_keys_are_rejected() {
    let error = parse("a: {x: 1, x: 2}\n").expect_err("duplicate keys are unsupported");
    assert_eq!(error.line, 1);
}
