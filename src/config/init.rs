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

const TEMPLATE: &str = r#"# openbmclapi 节点配置。
# 这里每一项都覆盖同名的环境变量；不需要的行可以留着注释，整个文件也可以不存在。

# 集群身份，由主控下发。
# cluster_id: "你的集群 ID"
# cluster_secret: "你的集群密钥"

# 单进程跑多个节点。每个节点要有自己的端口；存储后端是共用的，
# 所以文件只校验一次，其余节点直接上线。
# instances:
#   - cluster_id: "集群 ID A"
#     cluster_secret: "集群密钥 A"
#     port: 4000
#   - cluster_id: "集群 ID B"
#     cluster_secret: "集群密钥 B"
#     port: 4001

# 监听端口与对外地址。
# port: 4000
# cluster_ip: "203.0.113.10"   # 主控能拨通的地址或域名
# cluster_public_port: 4000
# bmclapi_base: "https://openbmclapi.bangbang93.com"

# 存储：单个后端……
# storage:
#   type: alist
#   options:
#     url: "https://alist.example.com"
#     username: "user"
#     password: "secret"
#
# ……或者一池远端源。本地 file 后端不能进池，所以至少列两个远端源。
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

# 同步调节。
# sync_memory_budget: 256   # 同步时同时缓冲的下载字节数（MiB）
# measure_redirect: true    # 是否把已存的测速对象 302 给客户端（每个源还能各自退出）
# measure_sizes: [0, 1, 2, 4, 8, 16, 32, 64, 128]   # 预置到后端的测速对象大小（MiB）；[] 关闭

# 日志。写到目录会把流按类型拆成 access.log、sync.log、error.log、agent.log，
# 轮转交给外部的 logrotate 之类。
# log_dir: "./logs"
# log_level: "info"
# log_format: "pretty"   # "json" 每行一个对象，给采集器用
# plain_log: false
# disable_access_log: false

# 可选功能。
# disable_sign: false
# enable_upnp: false
# byoc: false
# no_daemon: false
# no_fast_enable: false
# ssl_key: "/path/to/key.pem"
# ssl_cert: "/path/to/cert.pem"
"#;
