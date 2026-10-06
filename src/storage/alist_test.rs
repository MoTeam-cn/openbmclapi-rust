use super::*;

#[test]
fn a_missing_url_names_alist_not_webdav() {
    let error = match AlistStorage::new(&Value::Null) {
        Ok(_) => panic!("a source without a url must be rejected"),
        Err(e) => e,
    };
    assert!(
        error.to_string().contains("alist: url is required"),
        "the message must name the backend that was configured, got: {error}"
    );
}

#[test]
fn parses_durations() {
    assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
    assert_eq!(parse_duration("30m"), Some(Duration::from_secs(1800)));
    assert_eq!(parse_duration("45s"), Some(Duration::from_secs(45)));
    assert_eq!(parse_duration("2d"), Some(Duration::from_secs(172800)));
    assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
    assert_eq!(parse_duration("nonsense"), None);
}
