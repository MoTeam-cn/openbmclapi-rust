use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::MutexGuard;

use serde_json::Value;

use super::load;
use crate::config::env::ENV_LOCK;

static FILE_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

/// Serialise the tests and give them a known environment.
fn prepare() -> MutexGuard<'static, ()> {
    let guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("CLUSTER_ID", "env-id");
    std::env::set_var("CLUSTER_SECRET", "env-secret");
    for key in [
        "CLUSTER_PORT",
        "CLUSTER_PUBLIC_PORT",
        "CLUSTER_STORAGE",
        "CLUSTER_STORAGE_OPTIONS",
        "CLUSTER_BMCLAPI",
        "LOGLEVEL",
        "SPEEDTEST_SIZES",
    ] {
        std::env::remove_var(key);
    }
    guard
}

/// Write a unique temporary configuration and return its path.
fn write_config(name: &str, body: &str) -> PathBuf {
    let sequence = FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "openbmclapi-config-{}-{sequence}-{name}.yaml",
        std::process::id()
    ));
    std::fs::write(&path, body).expect("write the temporary configuration");
    path
}

fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

fn url_of(config: &crate::config::Config) -> Option<&str> {
    config
        .storage_opts
        .as_ref()
        .and_then(|options| options.get("url"))
        .and_then(Value::as_str)
}

#[test]
fn a_missing_file_keeps_the_environment_values() {
    let _guard = prepare();
    let missing = std::env::temp_dir().join("openbmclapi-config-absent.yaml");
    remove(&missing);
    let config = load(Some(&missing)).expect("load without a file");
    assert_eq!(config.cluster_id, "env-id");
    assert_eq!(config.port, crate::config::DEFAULT_PORT);
    assert_eq!(config.storage, "file");
    assert_eq!(config.storage_sources.len(), 1);
    assert_eq!(config.storage_sources[0].kind, "file");
}

#[test]
fn file_values_override_the_environment() {
    let _guard = prepare();
    std::env::set_var("CLUSTER_PORT", "4123");
    let path = write_config(
        "override",
        "cluster_id: \"file-id\"\nport: 5000\nlog_level: debug\n",
    );
    let config = load(Some(&path)).expect("load the file");
    assert_eq!(config.cluster_id, "file-id");
    assert_eq!(config.port, 5000);
    assert_eq!(config.log_level, "debug");
    assert_eq!(config.cluster_secret, "env-secret");
    remove(&path);
}

#[test]
fn a_single_storage_source_sets_the_legacy_fields() {
    let _guard = prepare();
    let path = write_config(
        "single",
        "storage:\n  type: alist\n  options: { url: \"https://a\", username: \"u\" }\n",
    );
    let config = load(Some(&path)).expect("load the file");
    assert_eq!(config.storage, "alist");
    assert_eq!(config.flavor.storage, "alist");
    assert_eq!(config.storage_sources.len(), 1);
    assert_eq!(config.storage_sources[0].kind, "alist");
    assert_eq!(url_of(&config), Some("https://a"));
    remove(&path);
}

#[test]
fn a_sources_list_becomes_the_storage_pool() {
    let _guard = prepare();
    let path = write_config(
        "pool",
        "storage:\n  sources:\n    - type: alist\n      options: { url: \"https://a\" }\n    - type: webdav\n      options: { url: \"https://b\" }\n",
    );
    let config = load(Some(&path)).expect("load the file");
    assert_eq!(config.storage_sources.len(), 2);
    assert_eq!(config.storage_sources[0].kind, "alist");
    assert_eq!(config.storage_sources[1].kind, "webdav");
    assert_eq!(config.storage, "alist");
    assert_eq!(url_of(&config), Some("https://a"));
    remove(&path);
}

#[test]
fn the_file_backend_cannot_join_a_sources_list() {
    let _guard = prepare();
    let path = write_config(
        "file-pool",
        "storage:\n  sources:\n    - type: file\n    - type: alist\n",
    );
    let error = load(Some(&path)).expect_err("the file backend must be rejected");
    assert!(
        error
            .to_string()
            .contains("cannot join a multi-source pool"),
        "unexpected error: {error}"
    );
    remove(&path);
}

#[test]
fn type_and_sources_together_are_rejected() {
    let _guard = prepare();
    let path = write_config(
        "conflict",
        "storage:\n  type: alist\n  sources:\n    - type: webdav\n",
    );
    let error = load(Some(&path)).expect_err("type and sources are exclusive");
    assert!(
        error.to_string().contains("mutually exclusive"),
        "unexpected error: {error}"
    );
    remove(&path);
}

#[test]
fn an_empty_sources_list_is_rejected() {
    let _guard = prepare();
    let path = write_config("empty-pool", "storage:\n  sources: []\n");
    let error = load(Some(&path)).expect_err("an empty pool is meaningless");
    assert!(
        error.to_string().contains("at least one source"),
        "unexpected error: {error}"
    );
    remove(&path);
}

#[test]
fn a_speed_test_list_can_be_a_sequence_or_a_string() {
    let _guard = prepare();

    let path = write_config("speedtest-seq", "speedtest_sizes: [1, 4, 16]\n");
    let config = load(Some(&path)).expect("load a sequence");
    assert_eq!(config.speedtest_sizes, vec![1, 4, 16]);
    remove(&path);

    let path = write_config("speedtest-str", "speedtest_sizes: \"2, 8\"\n");
    let config = load(Some(&path)).expect("load a string");
    assert_eq!(config.speedtest_sizes, vec![2, 8]);
    remove(&path);

    let path = write_config("speedtest-empty", "speedtest_sizes: []\n");
    let config = load(Some(&path)).expect("load an empty list");
    assert!(
        config.speedtest_sizes.is_empty(),
        "an empty list disables seeding"
    );
    remove(&path);
}
