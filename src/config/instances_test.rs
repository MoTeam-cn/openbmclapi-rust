use std::sync::MutexGuard;

use crate::config::env::ENV_LOCK;
use crate::config::Config;

/// A single-node configuration with a known environment.
fn base() -> MutexGuard<'static, ()> {
    let guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("CLUSTER_ID", "one");
    std::env::set_var("CLUSTER_SECRET", "secret-one");
    for key in [
        "CLUSTER_INSTANCES",
        "CLUSTER_PORT",
        "CLUSTER_PUBLIC_PORT",
        "CLUSTER_IP",
        "ENABLE_UPNP",
    ] {
        std::env::remove_var(key);
    }
    guard
}

fn config() -> Config {
    Config::from_env().expect("config")
}

#[test]
fn a_single_node_splits_into_itself() {
    let _guard = base();
    let config = config();
    let split = config.split().expect("split");
    assert_eq!(split.len(), 1);
    assert_eq!(split[0].cluster_id, "one");
    assert_eq!(split[0].cluster_secret, "secret-one");
}

#[test]
fn each_instance_gets_its_own_identity_and_port() {
    let _guard = base();
    std::env::set_var(
        "CLUSTER_INSTANCES",
        r#"[{"cluster_id":"a","cluster_secret":"sa","port":4000},
            {"cluster_id":"b","cluster_secret":"sb","port":4001}]"#,
    );
    std::env::remove_var("CLUSTER_ID");
    std::env::remove_var("CLUSTER_SECRET");
    let split = config().split().expect("split");
    assert_eq!(split.len(), 2);
    assert_eq!(split[0].cluster_id, "a");
    assert_eq!(split[1].cluster_id, "b");
    assert_eq!(split[1].port, 4001);
    assert_eq!(
        split[1].cluster_public_port, 4001,
        "the public port defaults to the listening port"
    );
    assert!(
        split.iter().all(|config| config.instances.is_empty()),
        "an expanded config looks like a single node"
    );
}

#[test]
fn an_explicit_public_port_is_kept() {
    let _guard = base();
    std::env::set_var(
        "CLUSTER_INSTANCES",
        r#"[{"cluster_id":"a","cluster_secret":"sa","port":4000,"cluster_public_port":9000}]"#,
    );
    std::env::remove_var("CLUSTER_ID");
    std::env::remove_var("CLUSTER_SECRET");
    let split = config().split().expect("split");
    assert_eq!(split[0].port, 4000);
    assert_eq!(split[0].cluster_public_port, 9000);
}

#[test]
fn two_instances_on_one_port_are_rejected() {
    let _guard = base();
    std::env::set_var(
        "CLUSTER_INSTANCES",
        r#"[{"cluster_id":"a","cluster_secret":"sa","port":4000},
            {"cluster_id":"b","cluster_secret":"sb","port":4000}]"#,
    );
    std::env::remove_var("CLUSTER_ID");
    std::env::remove_var("CLUSTER_SECRET");
    let error = config()
        .split()
        .expect_err("duplicate ports must be refused");
    assert!(error.to_string().contains("already used"), "{error}");
}

#[test]
fn an_instance_without_a_secret_is_rejected() {
    let _guard = base();
    std::env::set_var(
        "CLUSTER_INSTANCES",
        r#"[{"cluster_id":"a","cluster_secret":"","port":4000}]"#,
    );
    std::env::remove_var("CLUSTER_ID");
    std::env::remove_var("CLUSTER_SECRET");
    let error = config().split().expect_err("a secret is required");
    assert!(error.to_string().contains("cluster_secret"), "{error}");
}

#[test]
fn a_top_level_identity_and_instances_are_mutually_exclusive() {
    let _guard = base();
    // What a file that lists instances while the environment still carries an
    // identity looks like once both layers are merged.
    let mut config = config();
    config.instances.push(crate::config::Instance {
        cluster_id: "a".into(),
        cluster_secret: "sa".into(),
        port: 4000,
        cluster_public_port: None,
        cluster_ip: None,
    });
    let error = config.validate().expect_err("the two forms are exclusive");
    assert!(error.to_string().contains("mutually exclusive"), "{error}");
}

#[test]
fn a_missing_identity_is_rejected() {
    let _guard = base();
    std::env::remove_var("CLUSTER_ID");
    let error = config().validate().expect_err("an identity is required");
    assert!(error.to_string().contains("cluster_id"), "{error}");
}

#[test]
fn identity_variables_are_refused_alongside_instances() {
    let _guard = base();
    std::env::set_var(
        "CLUSTER_INSTANCES",
        r#"[{"cluster_id":"a","cluster_secret":"sa","port":4000}]"#,
    );
    let error = Config::from_env().expect_err("CLUSTER_ID must not be combined");
    assert!(error.to_string().contains("cannot be combined"), "{error}");
}

#[test]
fn each_instance_gets_its_own_temporary_directory() {
    let _guard = base();
    std::env::set_var(
        "CLUSTER_INSTANCES",
        r#"[{"cluster_id":"a","cluster_secret":"sa","port":4000},
            {"cluster_id":"b","cluster_secret":"sb","port":4001}]"#,
    );
    std::env::remove_var("CLUSTER_ID");
    std::env::remove_var("CLUSTER_SECRET");
    let split = config().split().expect("split");
    assert_ne!(
        split[0].tmp_dir(),
        split[1].tmp_dir(),
        "certificates would overwrite each other"
    );
}
