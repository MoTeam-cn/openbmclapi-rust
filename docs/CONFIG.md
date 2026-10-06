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
| `SSL_KEY` / `SSL_CERT` | – | PEM 文件路径或内联 PEM 内容（仅 BYOC）。 |
| `ENABLE_NGINX` | `false` | 在节点前挂 nginx：nginx 接管公网端口并从磁盘直接吐缓存，节点退到 loopback 端口。需要系统已安装 nginx。 |
| `ENABLE_UPNP` | `false` | 用 UPnP IGD 映射公网端口。 |
| `DISABLE_ACCESS_LOG` | `false` | 关闭逐请求访问日志。 |
| `DISABLE_SIGN` | `false` | 跳过 `s`/`e` 签名校验（仅限可信网络）。 |
| `NO_DAEMON` | `false` | 单进程运行，不拉起受监管 worker。 |
| `NO_FAST_ENABLE` | `false` | 要求主控跳过快速启用流程。 |
| `SYNC_MEMORY_BUDGET` | `256` | 同步时允许同时缓冲的下载字节数（MiB）。 |
| `MEASURE_SIZES` | `0,1,2,4,8,16,32,64,128` | 预置到远端后端的 measure 对象大小（MiB），逗号分隔；留空关闭预置。 |
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

## 多存储源

`storage` 可以写成一池远端源：

```yaml
storage:
  sources:
    - type: alist
      options: { url: "https://alist-a.example.com", username: "u", password: "p" }
    - type: webdav
      options: { url: "https://dav-b.example.com", username: "u", password: "p" }
```

- 读请求轮转分发，某个源失败或 404 时自动换下一个，全部失败才对外报错。
- 写入复制到所有源，某个源写失败会在下一轮同步补上。
- `exists` 要求每个源都有该对象，`get_missing_files` 取各源缺失的并集，
  `gc` 在每个源上分别执行后汇总计数。
- `type` / `options`（单源）与 `sources`（多源）互斥，同时出现直接报错。
- 池里不允许出现 `file`：本地磁盘是节点自己的缓存，不是远端镜像。

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