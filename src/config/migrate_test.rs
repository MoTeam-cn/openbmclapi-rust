use std::sync::MutexGuard;

use super::convert;
use crate::config::env::ENV_LOCK;
use crate::testutil::TempFile;

/// The Node agent's own `.env`, as an operator would hand it over.
const NODE_ENV: &str = "\
# 集群身份
CLUSTER_ID=demo
CLUSTER_SECRET=secret
CLUSTER_IP=bmcl-eo.moiu.cn
CLUSTER_PORT=4888
CLUSTER_PUBLIC_PORT=443
CLUSTER_BYOC=true
CLUSTER_STORAGE=alist
CLUSTER_STORAGE_OPTIONS={\"url\":\"http://127.0.0.1:5244/dav\",\"basePath\":\"Cache/download\"}
";

/// Serialise the tests and give them a known environment.
fn prepare() -> MutexGuard<'static, ()> {
    let guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("CLUSTER_ID", "env-id");
    std::env::set_var("CLUSTER_SECRET", "env-secret");
    for key in [
        "CLUSTER_INSTANCES",
        "CLUSTER_PORT",
        "CLUSTER_PUBLIC_PORT",
        "CLUSTER_IP",
        "CLUSTER_BYOC",
        "CLUSTER_STORAGE",
        "CLUSTER_STORAGE_OPTIONS",
        "CLUSTER_BMCLAPI",
        "MEASURE_SIZES",
    ] {
        std::env::remove_var(key);
    }
    guard
}

#[test]
fn maps_the_node_variables_onto_our_keys() {
    let yaml = convert(NODE_ENV, ".env").expect("conversion");
    assert!(yaml.contains("cluster_id: \"demo\""), "{yaml}");
    assert!(yaml.contains("cluster_secret: \"secret\""), "{yaml}");
    assert!(yaml.contains("cluster_ip: \"bmcl-eo.moiu.cn\""), "{yaml}");
    assert!(yaml.contains("port: 4888"), "{yaml}");
    assert!(yaml.contains("cluster_public_port: 443"), "{yaml}");
    assert!(yaml.contains("byoc: true"), "{yaml}");
}

#[test]
fn storage_becomes_a_block_and_the_options_stay_json() {
    let yaml = convert(NODE_ENV, ".env").expect("conversion");
    assert!(yaml.contains("storage:\n  type: \"alist\"\n"), "{yaml}");
    // Re-serialised through serde_json, so the keys come out sorted rather than
    // in the order the .env listed them. Object order carries no meaning here.
    assert!(
        yaml.contains(
            "  options: {\"basePath\":\"Cache/download\",\"url\":\"http://127.0.0.1:5244/dav\"}"
        ),
        "{yaml}"
    );
}

#[test]
fn a_key_with_no_counterpart_is_reported_not_dropped() {
    let yaml = convert("CLUSTER_ID=a\nSOME_NODE_THING=1\n", ".env").expect("conversion");
    assert!(yaml.contains("#   SOME_NODE_THING"), "{yaml}");
}

#[test]
fn the_converted_document_loads_as_our_own_configuration() {
    let _guard = prepare();
    let yaml = convert(NODE_ENV, ".env").expect("conversion");
    let file = TempFile::write("migrated.yaml", &yaml);
    let config = crate::config::load(Some(file.path())).expect("the converted file must load");
    assert_eq!(config.cluster_id, "demo");
    assert_eq!(config.port, 4888);
    assert_eq!(config.cluster_public_port, 443);
    assert!(config.byoc);
    assert_eq!(config.storage_sources[0].kind, "alist");
    assert_eq!(
        config.storage_sources[0]
            .options
            .get("basePath")
            .and_then(|v| v.as_str()),
        Some("Cache/download")
    );
}

#[test]
fn a_boolean_is_written_as_a_boolean_not_a_string() {
    let yaml = convert("CLUSTER_BYOC=0\n", ".env").expect("conversion");
    assert!(yaml.contains("byoc: false"), "{yaml}");
}

#[test]
fn sizes_become_a_flow_list() {
    let yaml = convert("MEASURE_SIZES=0,1,2\n", ".env").expect("conversion");
    assert!(yaml.contains("measure_sizes: [0, 1, 2]"), "{yaml}");
}

#[test]
fn a_non_numeric_port_is_refused() {
    let error = convert("CLUSTER_PORT=not-a-port\n", ".env").expect_err("must be refused");
    assert!(error.to_string().contains("需要一个整数"), "{error}");
}

#[test]
fn quoted_values_lose_their_quotes() {
    let yaml = convert("CLUSTER_ID=\"a b\"\n", ".env").expect("conversion");
    assert!(yaml.contains("cluster_id: \"a b\""), "{yaml}");
}
