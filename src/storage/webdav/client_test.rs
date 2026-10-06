use super::*;

#[test]
fn joins_urls() {
    assert_eq!(
        join_url("http://host/dav/", "ab/cd ef"),
        "http://host/dav/ab/cd%20ef"
    );
}
