use serde_json::Value;

use super::{Config, DEFAULT_BMCLAPI_BASE, DEFAULT_PORT, DEFAULT_SPEEDTEST_SIZES, ENV_LOCK};

/// Serialise the tests and give them a known environment.
fn prepare() -> std::sync::MutexGuard<'static, ()> {
    let guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("CLUSTER_ID", "test-id");
    std::env::set_var("CLUSTER_SECRET", "test-secret");
    for key in [
        "CLUSTER_PORT",
        "CLUSTER_PUBLIC_PORT",
        "CLUSTER_STORAGE",
        "CLUSTER_STORAGE_OPTIONS",
        "CLUSTER_BMCLAPI",
        "CLUSTER_IP",
        "LOGLEVEL",
        "SYNC_MEMORY_BUDGET",
        "SPEEDTEST_SIZES",
    ] {
        std::env::remove_var(key);
    }
    guard
}

#[test]
fn the_environment_fills_the_defaults() {
    let _guard = prepare();
    let config = Config::from_env().expect("environment configuration");
    assert_eq!(config.cluster_id, "test-id");
    assert_eq!(config.cluster_secret, "test-secret");
    assert_eq!(config.port, DEFAULT_PORT);
    assert_eq!(config.cluster_public_port, DEFAULT_PORT);
    assert_eq!(config.storage, "file");
    assert_eq!(config.storage_opts, None);
    assert_eq!(config.storage_sources.len(), 1);
    assert_eq!(config.storage_sources[0].kind, "file");
    assert!(config.storage_sources[0].options.is_null());
    assert_eq!(config.bmclapi_base, DEFAULT_BMCLAPI_BASE);
    assert_eq!(config.log_level, "info");
}

#[test]
fn explicit_variables_replace_the_defaults() {
    let _guard = prepare();
    std::env::set_var("CLUSTER_PORT", "4321");
    std::env::set_var("CLUSTER_STORAGE", "webdav");
    std::env::set_var("CLUSTER_STORAGE_OPTIONS", "{\"url\":\"https://dav\"}");
    let config = Config::from_env().expect("environment configuration");
    assert_eq!(config.port, 4321);
    assert_eq!(config.cluster_public_port, 4321);
    assert_eq!(config.storage, "webdav");
    assert_eq!(config.storage_sources.len(), 1);
    assert_eq!(config.storage_sources[0].kind, "webdav");
    assert_eq!(
        config
            .storage_opts
            .as_ref()
            .and_then(|options| options.get("url"))
            .and_then(Value::as_str),
        Some("https://dav")
    );
}

#[test]
fn the_speed_test_ladder_has_a_default_and_can_be_disabled() {
    let _guard = prepare();
    std::env::remove_var("SPEEDTEST_SIZES");
    let config = Config::from_env().expect("environment configuration");
    assert_eq!(config.speedtest_sizes, DEFAULT_SPEEDTEST_SIZES.to_vec());

    std::env::set_var("SPEEDTEST_SIZES", "");
    let config = Config::from_env().expect("environment configuration");
    assert!(
        config.speedtest_sizes.is_empty(),
        "an empty list disables seeding"
    );
}

#[test]
fn a_speed_test_size_list_is_parsed_and_deduplicated() {
    let _guard = prepare();
    std::env::set_var("SPEEDTEST_SIZES", " 4 , 1,4, 16 ");
    let config = Config::from_env().expect("environment configuration");
    assert_eq!(config.speedtest_sizes, vec![4, 1, 16]);
}

#[test]
fn an_unusable_speed_test_size_is_rejected() {
    let _guard = prepare();
    for raw in ["0", "201", "abc"] {
        std::env::set_var("SPEEDTEST_SIZES", raw);
        assert!(Config::from_env().is_err(), "{raw:?} must not be accepted");
    }
}
