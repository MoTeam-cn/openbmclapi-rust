use std::path::Path;

use super::join_key;

#[test]
fn joins_slash_separated_keys() {
    assert_eq!(
        join_key(Path::new("cache"), "ab/abcdef"),
        Path::new("cache/ab/abcdef")
    );
}

#[test]
fn rejects_parent_traversal() {
    assert_eq!(
        join_key(Path::new("cache"), "../../etc/passwd"),
        Path::new("cache/etc/passwd")
    );
}
