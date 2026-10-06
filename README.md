# openbmclapi-rust

OpenBMCLAPI 集群节点（`bmclapi@home`）的 Rust 实现，为
[openbmclapi.bangbang93.com](https://github.com/bangbang93/openbmclapi) 提供 Minecraft 文件镜像服务。

本项目是 Node.js 版 **v1.14.0** 的从零移植。上游 TypeScript 源码保留在 `openbmclapi/`
目录，仅作移植参考，**不属于**本仓库（见 `.gitignore`）。

## 节点做什么

1. 用 HMAC-SHA256 挑战/应答向主控认证。
2. 下载主控的文件清单（zstd 压缩的 Avro），校验 MD5 / SHA-1 后把缺失文件镜像到存储后端。
3. 从存储后端响应带签名的 `/download/:hash` 请求。
4. 每分钟通过 socket.io 上报已服务的字节数。
5. 清理已从主控文件清单中消失的对象。

## 构建

```bash
cargo build --release
# 产物：target/release/openbmclapi
```

需要 Rust 1.82 或更新版本。TLS 全部走纯 rustls：不需要 OpenSSL，也不需要系统证书工具。

## 运行

```bash
export CLUSTER_ID=你的集群ID
export CLUSTER_SECRET=你的集群密钥
./target/release/openbmclapi
```

默认会拉起一个受监管的 worker 进程，worker 退出时按指数退避重启。设 `NO_DAEMON=1`
可单进程前台运行，容器部署通常用这个。

工作目录下的 `.env` 会被自动加载。

### 命令行

```bash
openbmclapi                 # 运行节点（默认动作）
openbmclapi run             # 同上，显式写法
openbmclapi init            # 生成带注释的 config.yaml
openbmclapi init --force    # 覆盖已存在的文件
openbmclapi --config /etc/openbmclapi.yaml    # 指定配置文件
openbmclapi --help          # 全部参数
```

### 配置文件

除环境变量外，也可以用工作目录下的 `config.yaml` 配置，键名与环境变量一一对应：

```yaml
cluster_id: "your-cluster-id"
cluster_secret: "your-cluster-secret"
port: 4000
log_level: "info"
storage:
  type: alist
  options:
    url: "https://alist.example.com"
    username: "user"
    password: "secret"
```

规则：

- 配置文件里出现的键**覆盖**环境变量；文件不存在时行为与只有环境变量时完全一致。
- 未知的顶层键直接报错，拼错的键不会静默忽略。
- `--config` 指定的文件必须存在；默认的 `config.yaml` 则可有可无。

YAML 读取器是手写的（离线环境里没有可用的 YAML crate，与手写 Avro / SigV4 / engine.io 同理）。
支持注释、缩进映射、块序列、流式 `[a, b]` 与 `{a: 1}`、单双引号与转义、
null / 布尔 / 整数 / 浮点。**不支持**锚点与别名（`&` `*`）、标签（`!`）、
多行标量（`|` `>`）、文档分隔符（`---`）、merge key（`<<`）与重复键——
这些会带行号报错，不会静默误解。

### 多存储源

`storage` 也可以写成一池远端源：节点按**轮转**把下载分散到各源，某个源失败时自动切到
下一个；写入会复制到所有源，因此任何一份都能独立提供完整数据。

```yaml
storage:
  sources:
    - type: alist
      options: { url: "https://alist-a.example.com", username: "u", password: "p" }
    - type: webdav
      options: { url: "https://dav-b.example.com", username: "u", password: "p" }
```

- `type` / `options`（单源）与 `sources`（多源）互斥，同时出现直接报错。
- `sources` 至少一项，且**不允许出现 `file`**：本地磁盘是节点自己的缓存，
  不是远端镜像，混进来会让「数据在哪」有两种互相矛盾的含义。
- 多源下 `exists` 要求每个源都有该对象，`get_missing_files` 取各源缺失的并集，
  `gc` 在每个源上分别执行后汇总计数——所以某个源写失败会在下一轮同步自动补上。

### 环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `CLUSTER_ID` | **必填** | 主控下发的集群 ID。 |
| `CLUSTER_SECRET` | **必填** | 集群密钥，用于 HMAC 与下载签名。 |
| `CLUSTER_IP` | – | 无法自动探测时向主控上报的地址。 |
| `CLUSTER_PORT` | `4000` | 本地监听端口。 |
| `CLUSTER_PUBLIC_PORT` | 同 `CLUSTER_PORT` | 向主控上报的端口。 |
| `CLUSTER_BYOC` | `false` | 自带证书，不向主控申请。 |
| `CLUSTER_STORAGE` | `file` | 存储后端，见下节。 |
| `CLUSTER_STORAGE_OPTIONS` | – | 所选后端的 JSON 选项。 |
| `CLUSTER_BMCLAPI` | `https://openbmclapi.bangbang93.com` | 主控地址。 |
| `SSL_KEY` / `SSL_CERT` | – | PEM 文件路径或内联 PEM 内容（仅 BYOC）。 |
| `ENABLE_NGINX` | `false` | 在节点前挂 nginx。 |
| `ENABLE_UPNP` | `false` | 用 UPnP IGD 映射公网端口。 |
| `DISABLE_ACCESS_LOG` | `false` | 关闭逐请求访问日志。 |
| `DISABLE_SIGN` | `false` | 跳过 `s`/`e` 签名校验（仅限可信网络）。 |
| `NO_DAEMON` | `false` | 单进程运行，不拉起受监管 worker。 |
| `NO_FAST_ENABLE` | `false` | 要求主控跳过快速启用流程。 |
| `LOGLEVEL` | `info` | `trace` / `debug` / `info` / `warn` / `error`。 |
| `PLAIN_LOG` | `false` | 关闭 ANSI 颜色。 |
| `LOG_FORMAT` | `pretty` | `json` 时每行输出一个 JSON 对象，供采集器使用。 |
| `RUST_LOG` | – | 设了就覆盖 `LOGLEVEL`，可按模块细调（如 `openbmclapi::cluster=debug`）。 |

## 存储后端

`CLUSTER_STORAGE` 目前支持 **5 个**后端（也可以按上节配成一池多源）：

| 取值 | 后端 | 寻址 / 签名 | 说明 |
| --- | --- | --- | --- |
| `file` | 本地磁盘 | – | 对象落在工作目录的 `cache/<ab>/<hash>`。无需任何选项。 |
| `minio` | MinIO 及一切 S3 兼容对象存储 | path-style + 自实现 AWS SigV4 | 不依赖 `minio` SDK，支持预签名直链。 |
| `oss` | 阿里云 OSS | 自实现 Signature V1 | 不依赖 `ali-oss`，默认代理透传，可切直链重定向。 |
| `webdav` | 通用 WebDAV | 原生 PROPFIND / MKCOL / PUT / DELETE | 不依赖 `webdav` 包。 |
| `alist` | AList / OpenList 的 WebDAV | 同上 + 签名直链缓存 | 缓存落盘到 `cache/redirectUrl.json`，启动时载入。 |

### 各后端的 `CLUSTER_STORAGE_OPTIONS`

**`file`** —— 无选项。

**`minio`**

| 键 | 必填 | 说明 |
| --- | --- | --- |
| `url` | 是 | 形如 `https://key:secret@host:port/bucket/prefix?region=us-east-1`，凭据、bucket、prefix、region 全在这一条里。 |
| `internalUrl` | 否 | 内网地址，格式同上；用于主控侧请求，缺省回落到 `url`。 |

```jsonc
{ "url": "https://key:secret@minio.example.com:9000/bucket/prefix?region=us-east-1",
  "internalUrl": "http://minio.internal:9000/bucket/prefix?region=us-east-1" }
```

**`oss`**

| 键 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- |
| `accessKeyId` | 是 | – | AccessKey ID。 |
| `accessKeySecret` | 是 | – | AccessKey Secret。 |
| `bucket` | 是 | – | Bucket 名。 |
| `prefix` | 否 | 空 | 对象键前缀。 |
| `internal` | 否 | `false` | 用内网域名（`.aliyuncs.com` 换成 `-internal.aliyuncs.com`）。 |
| `proxy` | 否 | `true` | `true` 由节点流式透传；`false` 走直链重定向。 |
| `endpoint` | 否 | – | 直接指定 endpoint，覆盖按 `region` 推导的域名。 |
| `region` | 否 | `oss-cn-hangzhou` | 地域，可写 `oss-cn-hangzhou`、`cn-hangzhou` 或完整域名。 |
| `cname` | 否 | `false` | 使用自有 CNAME 域名，不加 `<bucket>.` 前缀。 |

```jsonc
{ "accessKeyId": "…", "accessKeySecret": "…", "bucket": "…", "prefix": "cache",
  "internal": false, "proxy": true, "region": "oss-cn-hangzhou", "cname": false }
```

**`webdav`**

| 键 | 必填 | 说明 |
| --- | --- | --- |
| `url` | 是 | WebDAV 根地址，例如 `https://dav.example.com/remote.php/dav`。 |
| `basePath` | 否 | 根地址下的子路径，例如 `/openbmclapi`。 |
| `username` | 否 | 用户名。 |
| `password` | 否 | 密码。 |

```jsonc
{ "url": "https://dav.example.com/remote.php/dav", "username": "u", "password": "p",
  "basePath": "/openbmclapi" }
```

**`alist`** —— 与 `webdav` 相同的四个键，外加：

| 键 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- |
| `cacheTtl` | 否 | 3600 秒 | 签名直链缓存时长。数字按**毫秒**解析，字符串按 `1h` / `30m` 这类时长解析。 |

```jsonc
{ "url": "http://alist.local:5244/dav", "username": "u", "password": "p",
  "basePath": "/openbmclapi", "cacheTtl": "1h" }
```

### 上游弹性

alist / OpenList 被打满时会连带把 agent 拖死（Node 版就崩在这里）。本移植在
`src/storage/resilience/` 里做了四件事，WebDAV 与 alist 两条路径共用：

| 机制 | 行为 |
| --- | --- |
| 自适应并发（AIMD） | 初始 8 并发，每成功一次 +1，上游报错则减半，区间 1–16。自己探到该实例的天花板，不用配置。 |
| 熔断器 | 连续 5 次失败后断开 15 秒，期间立刻返回失败；冷却后放一个探针请求。429 会**立刻**断开——那是 WebDAV 认证锁定，重试只会延长它。 |
| 有界重试 | 408/425/5xx 退避重试，最多 3 次，指数增长加抖动，上限 30 秒；带 `Retry-After` 时以它为准。 |
| 流式代理 | 响应体用 `Body::from_stream` 直接转发，不再整包读进内存——大文件不再撑爆进程。 |
| 源间故障转移 | 多源配置下，某个源失败（或 404）会自动换下一个源重试；全部失败才对外报错。 |

上游不可用时 `/download/{hash}` 返回 **503** 并带 `Retry-After`，客户端应当退避重试，
而不是继续压。连接池也已调优（`pool_max_idle_per_host` 从 reqwest 默认的无上限降到 16，
并设置空闲回收、TCP keepalive 与 `read_timeout`，避免长下载被总超时切断）。

## HTTP 接口

| 路由 | 用途 |
| --- | --- |
| `GET /download/{hash}` | 返回已缓存的对象。除设了 `DISABLE_SIGN` 外必须带合法 `s`/`e` 签名。未命中时回源主控拉取，并对并发请求去重、校验和。 |
| `GET /measure/{size}` | 带宽探针，流式返回 `size` MiB（上限 200）的 `0066ccff`。 |
| `GET /auth` | nginx `auth_request` 的校验端点，校验 `x-original-uri` 的签名。 |

监听端在 TLS（ALPN）下同时说 HTTP/2 与 HTTP/1.1，非 TLS 下为 HTTP/1.1，由 hyper 的
auto 驱动按连接协商。

## 架构

```
src/
  main.rs        二进制入口（仅委托给 daemon::entry）
  daemon.rs      守护进程：worker 重启与指数退避
  lib.rs         库根：只有模块声明与再导出
  bootstrap.rs   worker 启动流程：认证、证书、监听、端口检查、同步、启用
  cli.rs         命令行：run / init
  config/        配置：环境变量、YAML 文件、手写 YAML 读取器
  token.rs       HMAC 挑战/应答 + 后台令牌刷新
  client.rs      主控 HTTP 客户端（Bearer 认证、响应缓存）
  filelist.rs    手写 Avro 解码器，解析主控文件清单
  cluster/       注册、同步、下载、GC、计数器
  keepalive.rs   每分钟上报循环，出错自动重启
  socketio/      engine.io v4 / socket.io v4 客户端
  server.rs      hyper auto（h1/h2）监听 + rustls 配置
  routes/        /auth、/download/{hash}、/measure/{size}
  storage/       file、minio（S3 SigV4）、oss（阿里云 V1）、webdav、alist
                 multi.rs     多源池：读轮转、写复制、源间故障转移
                 resilience/  熔断、自适应并发、有界重试
                 shared/      两套云后端共用的 XML 与对象键辅助
  upnp/          SSDP 发现 + SOAP 端口映射
  nginx.rs       可选的 nginx 前置
  tls.rs         rustls 用的 PEM 解析
  types.rs       共享数据类型
  util.rs        哈希、大小与 Range 辅助
  error.rs       错误类型
  logger.rs      tracing 初始化
```

## 测试

```bash
cargo test --offline --lib
cargo test --offline --test http_surface
```

单元测试覆盖 Avro 解码、签名校验、Range 运算、multistatus 解析、nginx 模板渲染、时长解析
以及两套云签名实现。`tests/http_surface.rs` 用 file 后端起真实路由，覆盖签名下载、错误
签名、404、Range、measure 端点与 nginx auth 端点。

## 代码规范

本仓库遵循 [CLAUDE.md](CLAUDE.md)。几条硬性的：

- 单个源文件不超过 350 行；301–350 必须写明不拆的理由。
- `lib.rs` / `main.rs` / 任何 `mod.rs` 只能是入口：模块文档、`mod` 声明与再导出，逻辑放同级文件。
- 单元测试放同级 `<module>_test.rs`，用 `#[cfg(test)] #[path = "..."] mod tests;` 引入；跨模块测试放 `tests/`。
- 复用优先：同一段逻辑出现第二次就必须提取。
- 验证范围化，禁止 `--all-targets`：`cargo check --offline --lib`、`cargo clippy --offline --lib`、
  `cargo test --offline --lib <module>::`。全量只在收口时跑一次。

## 与 Node 版的差异

完整映射与每一处刻意偏离见 [docs/PORTING.md](docs/PORTING.md)。要点：

- 向主控上报的 runtime 是 `Rust/<版本>`，并额外带 `implementation` 与 `repo`，
  主控可以据此区分本移植与 Node 版；keep-alive 上报体仍是 `time`/`hits`/`bytes`。
- 配置支持 YAML 文件（`openbmclapi init` 生成）与多存储源，Node 版只有环境变量与单源。
- 对象键在所有平台统一用 `/` 分隔，远端键跨平台完全一致（Node 版用宿主分隔符）。
- nginx 反代到 loopback TCP 端口而非 unix socket，因此在 Windows 上也能用。
- 阿里云 OSS 与 S3 直接实现（Signature V1 / SigV4），不经过厂商 SDK。

## 许可证

MIT，见 [LICENSE](LICENSE)。移植自
[bangbang93/openbmclapi](https://github.com/bangbang93/openbmclapi)，同样为 MIT。
