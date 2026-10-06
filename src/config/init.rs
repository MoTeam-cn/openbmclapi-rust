//! The configuration skeleton written by the init subcommand.

/// Commented YAML skeleton covering every key the loader understands.
///
/// The text is written verbatim, so it must stay valid YAML: every line is a
/// comment until the operator uncomments it.
pub(crate) fn template() -> &'static str {
    TEMPLATE
}

/// Alias used by the `init` subcommand in the CLI module.
pub(crate) fn init_template() -> &'static str {
    template()
}

const TEMPLATE: &str = r#"# openbmclapi agent configuration.
# Every value here overrides the matching CLUSTER_* environment variable.
# Uncomment the lines you need; this file may also be absent entirely.

# Cluster identity, issued by the master.
# cluster_id: "your-cluster-id"
# cluster_secret: "your-cluster-secret"

# Listening port and advertised address.
# port: 4000
# cluster_ip: "203.0.113.10"
# cluster_public_port: 4000
# bmclapi_base: "https://openbmclapi.bangbang93.com"

# Storage: either a single backend ...
# storage:
#   type: alist
#   options:
#     url: "https://alist.example.com"
#     username: "user"
#     password: "secret"
#
# ... or a pool of several remote backends. The local file backend cannot
# join a pool, so list at least two remote sources.
# storage:
#   sources:
#     - type: alist
#       options:
#         url: "https://alist.example.com"
#         username: "user"
#         password: "secret"
#     - type: webdav
#       options:
#         url: "https://dav.example.com"
#         username: "user"
#         password: "secret"

# Logging.
# log_level: "info"
# plain_log: false
# disable_access_log: false

# Optional features.
# disable_sign: false
# enable_nginx: false
# enable_upnp: false
# byoc: false
# no_daemon: false
# no_fast_enable: false
# ssl_key: "/path/to/key.pem"
# ssl_cert: "/path/to/cert.pem"
"#;
