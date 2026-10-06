use tracing::Level;

use super::{is_access, is_agent, is_error, is_sync, open, ACCESS_TARGET};

#[test]
fn request_lines_go_to_the_access_file_only() {
    assert!(is_access(ACCESS_TARGET));
    assert!(!is_access("openbmclapi::cluster::sync"));
    assert!(!is_agent(ACCESS_TARGET), "an access line is not duplicated");
}

#[test]
fn sync_activity_goes_to_the_sync_file_only() {
    assert!(is_sync("openbmclapi::cluster::sync"));
    assert!(is_sync("openbmclapi::cluster::sync::inner"));
    assert!(!is_sync("openbmclapi::cluster::cluster"));
    assert!(!is_agent("openbmclapi::cluster::sync"));
}

#[test]
fn warnings_and_errors_are_mirrored_into_the_error_file() {
    assert!(is_error(&Level::WARN));
    assert!(is_error(&Level::ERROR));
    assert!(!is_error(&Level::INFO));
    assert!(!is_error(&Level::DEBUG));
}

#[test]
fn everything_else_lands_in_the_agent_file() {
    assert!(is_agent("openbmclapi::bootstrap"));
    assert!(is_agent("openbmclapi::cluster::cluster"));
}

#[test]
fn opening_creates_one_file_per_category() {
    let dir = std::env::temp_dir().join(format!("openbmclapi-logs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    open(&dir).expect("the directory is writable");

    for name in ["access.log", "sync.log", "error.log", "agent.log"] {
        assert!(dir.join(name).exists(), "{name} must be created");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
