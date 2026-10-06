use std::sync::MutexGuard;

use serde_json::Value;

use super::load;
use crate::config::env::ENV_LOCK;
use crate::testutil::{TempDir, TempFile};

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
        "MEASURE_SIZES",
    ] {
        std::env::remove_var(key);
    }
    guard
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
    let dir = TempDir::new("config");
    let missing = dir.path().join("absent.yaml");
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
    let file = TempFile::write(
        "config.yaml",
        "cluster_id: \"file-id\"\nport: 5000\nlog_level: debug\n",
    );
    let config = load(Some(file.path())).expect("load the file");
    assert_eq!(config.cluster_id, "file-id");
    assert_eq!(config.port, 5000);
    assert_eq!(config.log_level, "debug");
    assert_eq!(config.cluster_secret, "env-secret");
}

#[test]
fn a_single_storage_source_sets_the_legacy_fields() {
    let _guard = prepare();
    let file = TempFile::write(
        "config.yaml",
        "storage:\n  type: alist\n  options: { url: \"https://a\", username: \"u\" }\n",
    );
    let config = load(Some(file.path())).expect("load the file");
    assert_eq!(config.storage, "alist");
    assert_eq!(config.flavor.storage, "alist");
    assert_eq!(config.storage_sources.len(), 1);
    assert_eq!(config.storage_sources[0].kind, "alist");
    assert_eq!(url_of(&config), Some("https://a"));
}

#[test]
fn a_sources_list_becomes_the_storage_pool() {
    let _guard = prepare();
    let file = TempFile::write(
        "config.yaml",
        "storage:\n  sources:\n    - type: alist\n      options: { url: \"https://a\" }\n    - type: webdav\n      options: { url: \"https://b\" }\n",
    );
    let config = load(Some(file.path())).expect("load the file");
    assert_eq!(config.storage_sources.len(), 2);
    assert_eq!(config.storage_sources[0].kind, "alist");
    assert_eq!(config.storage_sources[1].kind, "webdav");
    assert_eq!(config.storage, "alist");
    assert_eq!(url_of(&config), Some("https://a"));
}

#[test]
fn the_file_backend_cannot_join_a_sources_list() {
    let _guard = prepare();
    let file = TempFile::write(
        "config.yaml",
        "storage:\n  sources:\n    - type: file\n    - type: alist\n",
    );
    let error = load(Some(file.path())).expect_err("the file backend must be rejected");
    assert!(
        error
            .to_string()
            .contains("cannot join a multi-source pool"),
        "unexpected error: {error}"
    );
}

#[test]
fn type_and_sources_together_are_rejected() {
    let _guard = prepare();
    let file = TempFile::write(
        "config.yaml",
        "storage:\n  type: alist\n  sources:\n    - type: webdav\n",
    );
    let error = load(Some(file.path())).expect_err("type and sources are exclusive");
    assert!(
        error.to_string().contains("mutually exclusive"),
        "unexpected error: {error}"
    );
}

#[test]
fn an_empty_sources_list_is_rejected() {
    let _guard = prepare();
    let file = TempFile::write("config.yaml", "storage:\n  sources: []\n");
    let error = load(Some(file.path())).expect_err("an empty pool is meaningless");
    assert!(
        error.to_string().contains("at least one source"),
        "unexpected error: {error}"
    );
}

#[test]
fn the_log_format_can_come_from_the_file() {
    let _guard = prepare();
    let file = TempFile::write("config.yaml", "log_format: \"json\"\n");
    let config = load(Some(file.path())).expect("load");
    assert_eq!(config.log_format, "json");
}

#[test]
fn instances_in_the_file_expand_into_one_config_each() {
    let _guard = prepare();
    std::env::remove_var("CLUSTER_ID");
    std::env::remove_var("CLUSTER_SECRET");
    let file = TempFile::write(
        "config.yaml",
        "instances:\n  - cluster_id: \"a\"\n    cluster_secret: \"sa\"\n    port: 4000\n  - cluster_id: \"b\"\n    cluster_secret: \"sb\"\n    port: 4001\n",
    );
    let split = load(Some(file.path()))
        .expect("load")
        .split()
        .expect("split");
    assert_eq!(split.len(), 2);
    assert_eq!(split[0].cluster_id, "a");
    assert_eq!(split[1].port, 4001);
    assert_eq!(split[1].cluster_public_port, 4001);
}

#[test]
fn instances_in_the_file_conflict_with_the_environment_identity() {
    let _guard = prepare();
    let file = TempFile::write(
        "config.yaml",
        "instances:\n  - cluster_id: \"a\"\n    cluster_secret: \"sa\"\n    port: 4000\n",
    );
    let error = load(Some(file.path())).expect_err("the two forms are exclusive");
    assert!(error.to_string().contains("mutually exclusive"), "{error}");
}

#[test]
fn a_measure_list_can_be_a_sequence_or_a_string() {
    let _guard = prepare();

    let file = TempFile::write("config.yaml", "measure_sizes: [1, 4, 16]\n");
    let config = load(Some(file.path())).expect("load a sequence");
    assert_eq!(config.measure_sizes, vec![1, 4, 16]);

    let file = TempFile::write("config.yaml", "measure_sizes: \"2, 8\"\n");
    let config = load(Some(file.path())).expect("load a string");
    assert_eq!(config.measure_sizes, vec![2, 8]);

    let file = TempFile::write("config.yaml", "measure_sizes: []\n");
    let config = load(Some(file.path())).expect("load an empty list");
    assert!(
        config.measure_sizes.is_empty(),
        "an empty list disables seeding"
    );
}
