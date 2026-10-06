use super::Error;

#[test]
fn a_configuration_error_is_fatal() {
    assert!(
        Error::Config("no url".into()).is_fatal(),
        "restarting cannot fix a bad configuration"
    );
}

#[test]
fn a_transient_error_is_not_fatal() {
    assert!(!Error::storage("the backend is down").is_fatal());
    assert!(!Error::UpstreamUnavailable { retry_in_ms: 10 }.is_fatal());
    assert!(!Error::NotFound.is_fatal());
    assert!(!Error::Timeout("slow".into()).is_fatal());
}
