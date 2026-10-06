# 配置参考

## 环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `CLUSTER_ID` | **必填** | 主控下发的集群 ID。 |
| `CLUSTER_SECRET` | **必填** | 集群密钥，用于 HMAC 与下载签名。 |
| `CLUSTER_IP` | – | 无法自动探测时向主控上报的地址。 |
| `CLUSTER_PORT` | `4000` | 本地监听端口。 |
| `CLUSTER_PUBLIC_PORT` | 同 `CLUSTER_PORT` | 向主控上报的端口。 |
| `CLUSTER_BYOC` | `false` | 自带证书，不向主控申请。 |
| `CLUSTER_STORAGE` | `file` | 存储后端。 |
| `CLUSTER_STORAGE_OPTIONS` | – | 所选后端的 JSON 选项。 |
| `CLUSTER_BMCLAPI` | `https://openbmclapi.bangbang93.com` | 主控地址。 |
| `CLUSTER_INSTANCES` | – | 多个节点身份的 JSON 数组。设了它就不能再设 `CLUSTER_ID` / `CLUSTER_SECRET` / `CLUSTER_PORT` / `CLUSTER_PUBLIC_PORT` / `CLUSTER_IP`。 |
| `SSL_KEY` / `SSL_CERT` | – | PEM 文件路径或内联 PEM 内容（仅 BYOC）。 |
| `ENABLE_UPNP` | `false` | 用 UPnP IGD 映射公网端口。 |
| `DISABLE_ACCESS_LOG` | `false` | 关闭逐请求访问日志。 |
| `DISABLE_SIGN` | `false` | 跳过 `s`/`e` 签名校验（仅限可信网络）。 |
| `NO_DAEMON` | `false` | 单进程运行，不拉起受监管 worker。 |
| `NO_FAST_ENABLE` | `false` | 要求主控跳过快速启用流程。 |
| `SYNC_MEMORY_BUDGET` | `256` | 同步时允许同时缓冲的下载字节数（MiB）。 |
| `MEASURE_SIZES` | `0,1,2,4,8,16,32,64,128` | 预置到远端后端的 measure 对象大小（MiB），逗号分隔；留空关闭预置。 |
| `MEASURE_REDIRECT` | `true` | 是否把已存的测速对象 302 给客户端；关掉则节点自己生成。每个源还能各自退出。 |
| `LOGLEVEL` | `info` | `trace` / `debug` / `info` / `warn` / `error`。 |
| `PLAIN_LOG` | `false` | 关闭 ANSI 颜色。 |
| `LOG_FORMAT` | `pretty` | `json` 时每行输出一个 JSON 对象，供采集器使用。 |
| `LOG_DIR` | – | 按类型分文件写日志：`access.log` / `sync.log` / `error.log` / `agent.log`。 |
| `RUST_LOG` | – | 覆盖 `LOGLEVEL`，可按模块细调（如 `openbmclapi::cluster=debug`）。 |

## YAML 文件

`config.yaml` 的键名与环境变量一一对应，文件里出现的键覆盖环境变量；文件不存在时
行为与只有环境变量时完全一致。未知的顶层键直接报错，拼错的键不会静默忽略。
`--config` 指定的文件必须存在，默认的 `config.yaml` 可有可无。

`openbmclapi init` 会生成一份带注释的骨架，覆盖所有键。

YAML 读取器是手写的。支持注释、缩进映射、块序列、流式 `[a, b]` 与 `{a: 1}`、
单双引号与转义、null / 布尔 / 整数 / 浮点。不支持锚点与别名（`&` `*`）、
标签（`!`）、多行标量（`|` `>`）、文档分隔符（`---`）、merge key（`<<`）
与重复键；遇到这些会带行号报错，不会静默误解。

## 多实例

一份配置可以跑多个节点身份，它们**共用一个存储后端**：文件只校验一遍，第一个实例跑完
之后其余实例直接请求上线。

```yaml
instances:
  - cluster_id: "id-a"
    cluster_secret: "secret-a"
    port: 4000
  - cluster_id: "id-b"
    cluster_secret: "secret-b"
    port: 4001
```

等价的环境变量写法（JSON 数组）：

```bash
CLUSTER_INSTANCES='[{"cluster_id":"id-a","cluster_secret":"secret-a","port":4000},
                    {"cluster_id":"id-b","cluster_secret":"secret-b","port":4001}]'
```

规则：

- 每个实例必须有 `cluster_id`、`cluster_secret` 和独立的 `port`；端口重复直接报错。
- `cluster_public_port` 缺省等于 `port`，`cluster_ip` 缺省自动探测。
- `instances` 与顶层的 `cluster_id` / `cluster_secret` / `port` /
  `cluster_public_port` / `cluster_ip` **互斥**，同时出现直接报错。
- 多个实例不能和 `ENABLE_UPNP` 一起用：它只认一个公网端口。
- 每个实例的证书放在各自的临时目录里（按 cluster_id 分），互不覆盖。

## 从 Node 的 .env 迁移

`openbmclapi migrate` 直接读 Node 版那份 `.env`，写出本程序能读的 `config.yaml`：

```bash
./openbmclapi migrate              # 读当前目录的 .env，写 config.yaml
./openbmclapi migrate .env.prod    # 指定文件
cat .env | ./openbmclapi migrate - # 从标准输入读
./openbmclapi migrate --force      # 覆盖已存在的 config.yaml
```

转换是逐键的，没有对应项的键会在输出文件末尾以注释列出，不会被悄悄丢掉。`.env` 文件
本身仍然会被读取，作为 YAML 之下的兜底；命令行上的环境变量优先级最高。

键名一一对应，环境变量换成 YAML 的 snake_case；`storage.options` 里的键名保持上游的
camelCase 写法不变。

| 环境变量 | YAML 键 |
| --- | --- |
| `CLUSTER_ID` / `CLUSTER_SECRET` | `cluster_id` / `cluster_secret` |
| `CLUSTER_IP` | `cluster_ip`（地址或域名） |
| `CLUSTER_PORT` / `CLUSTER_PUBLIC_PORT` | `port` / `cluster_public_port` |
| `CLUSTER_BYOC` | `byoc` |
| `CLUSTER_STORAGE` / `CLUSTER_STORAGE_OPTIONS` | `storage.type` / `storage.options` |
| `CLUSTER_BMCLAPI` | `bmclapi_base` |
| `ENABLE_UPNP` | `enable_upnp` |
| `DISABLE_SIGN` / `DISABLE_ACCESS_LOG` | `disable_sign` / `disable_access_log` |
| `NO_DAEMON` / `NO_FAST_ENABLE` | `no_daemon` / `no_fast_enable` |
| `SSL_KEY` / `SSL_CERT` | `ssl_key` / `ssl_cert` |
| `LOGLEVEL` / `PLAIN_LOG` / `LOG_FORMAT` / `LOG_DIR` | `log_level` / `plain_log` / `log_format` / `log_dir` |
| `SYNC_MEMORY_BUDGET` / `MEASURE_SIZES` / `MEASURE_REDIRECT` | `sync_memory_budget` / `measure_sizes` / `measure_redirect` |
| `CLUSTER_INSTANCES` | `instances` |

例如这一份 .env：

```bash
CLUSTER_ID=x
CLUSTER_SECRET=x
CLUSTER_BYOC=true
CLUSTER_PUBLIC_PORT=443
CLUSTER_IP=bmcl-eo.moiu.cn
CLUSTER_PORT=4888
CLUSTER_STORAGE=alist
CLUSTER_STORAGE_OPTIONS={"url":"http://127.0.0.1:5244/dav","basePath":"Cache/download","username":"openbmclapi","password":"openbmclapi"}
```

等价于：

```yaml
cluster_id: "x"
cluster_secret: "x"
byoc: true
port: 4888
cluster_public_port: 443
cluster_ip: "bmcl-eo.moiu.cn"
storage:
  type: alist
  options:
    url: "http://127.0.0.1:5244/dav"
    basePath: "Cache/download"
    username: "openbmclapi"
    password: "openbmclapi"
```

`byoc: true` 但没有 `ssl_cert` / `ssl_key` 时，节点按 HTTP 提供服务，由前面的反向代理
终止 TLS——和 Node 版一致。`basePath` 带不带前导斜杠都行。

## 节点地址与域名

Agent 不碰域名，它只做两件事：

1. 在 `port-check` 和 `enable` 里上报 `host` + `port`，取值来自 `cluster_ip` 与
   `cluster_public_port`。
2. 用 `request-cert` 向主控要证书——这个请求**不带任何参数**，主控凭认证过的集群身份
   决定证书签给谁。

域名归主控：用主控分配的子域还是绑自己的域名，都在主控那边设置；它把名字指向哪里，
取决于你上报的 `host` + `port`。

- `cluster_ip` 不填时不上报该字段，主控按连接的来源地址自行判断。
- 填了就用你给的值，**可以是域名而不限于 IP**（例如 `node-a.example.com`），
  主控拿它做端口探测。
- 多实例时每个实例各带 `cluster_ip` / `cluster_public_port`，
  所以多个域名可以分别指向不同实例。
- 开了 `ENABLE_UPNP` 时，探测到的公网 IP 会覆盖 `cluster_ip`。

## 多存储源

`storage` 可以写成一池远端源：

```yaml
storage:
  sources:
    - type: alist
      options: { url: "https://alist-a.example.com", username: "u", password: "p" }
    - type: webdav
      measure_redirect: false
      options: { url: "https://dav-b.example.com", username: "u", password: "p" }
```

- 读请求轮转分发，某个源失败或 404 时自动换下一个，全部失败才对外报错。
- 写入复制到所有源，某个源写失败会在下一轮同步补上。
- `exists` 要求每个源都有该对象，`get_missing_files` 取各源缺失的并集，
  `gc` 在每个源上分别执行后汇总计数。
- `type` / `options`（单源）与 `sources`（多源）互斥，同时出现直接报错。
- 池里不允许出现 `file`：本地磁盘是节点自己的缓存，不是远端镜像。
- 每个源可以写 `measure_redirect: false`，退出测速对象的 302 应答（见下节）。

## 存储后端选项

### `file`

无选项。

### `minio`

| 键 | 必填 | 说明 |
| --- | --- | --- |
| `url` | 是 | 形如 `https://key:secret@host:port/bucket/prefix?region=us-east-1`，凭据、bucket、prefix、region 全在这一条里。 |
| `internalUrl` | 否 | 内网地址，格式同上；用于主控侧请求，缺省回落到 `url`。 |

```jsonc
{ "url": "https://key:secret@minio.example.com:9000/bucket/prefix?region=us-east-1",
  "internalUrl": "http://minio.internal:9000/bucket/prefix?region=us-east-1" }
```

### `oss`

| 键 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- |
| `accessKeyId` | 是 | – | AccessKey ID。 |
| `accessKeySecret` | 是 | – | AccessKey Secret。 |
| `bucket` | 是 | – | Bucket 名。 |
| `prefix` | 否 | 空 | 对象键前缀。 |
| `internal` | 否 | `false` | 用内网域名。 |
| `proxy` | 否 | `true` | `true` 由节点流式透传；`false` 走直链重定向。 |
| `endpoint` | 否 | – | 直接指定 endpoint，覆盖按 `region` 推导的域名。 |
| `region` | 否 | `oss-cn-hangzhou` | 地域，可写 `oss-cn-hangzhou`、`cn-hangzhou` 或完整域名。 |
| `cname` | 否 | `false` | 使用自有 CNAME 域名，不加 `<bucket>.` 前缀。 |

```jsonc
{ "accessKeyId": "…", "accessKeySecret": "…", "bucket": "…", "prefix": "cache",
  "internal": false, "proxy": true, "region": "oss-cn-hangzhou", "cname": false }
```

### `webdav`

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

### `alist`

与 `webdav` 相同的四个键，外加：

| 键 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- |
| `cacheTtl` | 否 | 3600 秒 | 签名直链缓存时长。数字按毫秒解析，字符串按 `1h` / `30m` 这类时长解析。 |

```jsonc
{ "url": "http://alist.local:5244/dav", "username": "u", "password": "p",
  "basePath": "/openbmclapi", "cacheTtl": "1h" }
```

## measure 对象

`/measure/{size}` 的探测数据来自存储后端，只有远端后端会被预置：

| 后端 | 行为 |
| --- | --- |
| `file` | 不预置，接口实时生成。 |
| 其他 | 启动时把 `MEASURE_SIZES` 列出的对象上传到保留文件夹 `measure/`，对象名是裸数字（`measure/0`、`measure/10`）。 |

- 命中的大小由后端应答，webdav / alist / S3 / OSS 返回后端直链（302）。
- `measure/0` 是 0 字节占位对象，主控的存活探测会请求它。
- `measure/` 是保留区，GC 会跳过。

### 谁来应答探测

两层开关，都同意才会 302：

| 层 | 键 | 缺省 |
| --- | --- | --- |
| 全局 | `measure_redirect` | `true` |
| 单源 | 源条目里的 `measure_redirect` | `true` |

解析顺序：

- 全局关掉 → 一律节点自己生成，不看任何源。
- 全局开着 → 在**开了的源**里找一个有该对象的 302 过去；轮转起点会轮换，多个源分摊探测。
- 全局开着但没有任何源同意（例如只有一个源且它关掉了）→ 视为没有 → 节点自己生成。
- 多个源一个开一个关 → 302 到开了的那个。

关掉只影响**谁来应答**。`MEASURE_SIZES` 列出的对象仍然会复制到每个源，因为检查阶段
要求每个源都齐；这也是这套配置里最占空间的部分（默认九档合计 255 MiB／源）。
