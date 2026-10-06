use super::{parse, unquote};

#[test]
fn a_json_value_keeps_its_inner_quotes() {
    let pairs = parse(r#"CLUSTER_STORAGE_OPTIONS={"url":"http://x"}"#);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].1, r#"{"url":"http://x"}"#);
}

#[test]
fn a_value_wrapped_as_a_whole_loses_its_quotes() {
    assert_eq!(unquote("\"a b\""), "a b");
    assert_eq!(unquote("'a b'"), "a b");
    assert_eq!(unquote("bare"), "bare");
}

#[test]
fn an_inline_comment_is_dropped_from_an_unquoted_value() {
    assert_eq!(unquote("4000 # the port"), "4000");
    assert_eq!(unquote("\"a # b\""), "a # b");
}

#[test]
fn comments_blanks_and_export_are_ignored() {
    let pairs = parse("# a comment\n\nexport FOO=1\nBAR=2\n");
    assert_eq!(
        pairs,
        vec![
            ("FOO".to_string(), "1".to_string()),
            ("BAR".to_string(), "2".to_string())
        ]
    );
}

#[test]
fn a_line_without_an_equals_is_skipped() {
    assert!(parse("JUST_A_WORD\n").is_empty());
}
