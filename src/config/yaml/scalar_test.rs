use serde_json::{json, Value};

use super::infer;
use crate::config::yaml::block::parse;

#[test]
fn plain_scalars_take_their_core_schema_type() {
    assert_eq!(infer(""), Value::Null);
    assert_eq!(infer("~"), Value::Null);
    assert_eq!(infer("null"), Value::Null);
    assert_eq!(infer("true"), Value::Bool(true));
    assert_eq!(infer("False"), Value::Bool(false));
    assert_eq!(infer("42"), json!(42));
    assert_eq!(infer("-7"), json!(-7));
    assert_eq!(infer("3.5"), json!(3.5));
    assert_eq!(infer("1e3"), json!(1000.0));
    assert_eq!(infer("1.14.0"), json!("1.14.0"));
    assert_eq!(infer("hello world"), json!("hello world"));
}

#[test]
fn quoted_scalars_honour_their_escapes() {
    let value = parse("a: \"line\\nbreak\"\nb: 'it''s'\nc: \"\\u0041\"\n").expect("parse");
    assert_eq!(value, json!({"a": "line\nbreak", "b": "it's", "c": "A"}));
}

#[test]
fn a_single_quoted_scalar_does_not_process_escapes() {
    let value = parse("a: 'line\\nbreak'\n").expect("parse");
    assert_eq!(value, json!({"a": "line\\nbreak"}));
}

#[test]
fn an_unknown_escape_is_rejected_with_its_line() {
    let error = parse("a: 1\nb: \"\\q\"\n").expect_err("unknown escapes are unsupported");
    assert_eq!(error.line, 2);
}

#[test]
fn an_unterminated_quote_is_rejected() {
    let error = parse("a: \"unterminated\n").expect_err("quotes must close");
    assert_eq!(error.line, 1);
}
