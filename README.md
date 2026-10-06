# openbmclapi-rust

[OpenBMCLAPI](https://github.com/bangbang93/openbmclapi) 集群节点的 Rust 实现，用来给
Minecraft 文件做镜像。移植自上游 Node 版 v1.14.0。

节点做四件事：向主控认证、把主控的文件清单镜像到存储后端、响应带签名的下载请求、
每分钟上报流量。

## 构建

```bash
cargo build --release
# 产物：target/release/openbmclapi
```

需要 Rust 1.82 或更新。TLS 走 rustls，不需要 OpenSSL。

## 运行

```bash
openbmclapi init          # 生成带注释的 config.yaml
openbmclapi               # 启动节点
openbmclapi --config /etc/openbmclapi.yaml
openbmclapi --help
```

不用配置文件也行，直接给环境变量：

```bash
CLUSTER_ID=xxx CLUSTER_SECRET=yyy ./openbmclapi
```

默认会拉起一个受监管的 worker，worker 退出时按指数退避重启；容器里一般用
`NO_DAEMON=1` 单进程前台运行。工作目录下的 `.env` 会被自动加载。

## 配置

`config.yaml` 的键名与环境变量一一对应，文件里写了的值覆盖环境变量，文件不存在时
行为与只用环境变量一样。

```yaml
cluster_id: "your-cluster-id"
cluster_secret: "your-cluster-secret"
port: 4000
storage:
  type: alist
  options:
    url: "https://alist.example.com"
    username: "user"
    password: "secret"
```

常用的环境变量：

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `CLUSTER_ID` | 必填 | 主控下发的集群 ID。 |
| `CLUSTER_SECRET` | 必填 | 集群密钥。 |
| `CLUSTER_PORT` | `4000` | 监听端口。 |
| `CLUSTER_STORAGE` | `file` | 存储后端，见下节。 |
| `CLUSTER_STORAGE_OPTIONS` | – | 后端的 JSON 选项。 |
| `CLUSTER_BMCLAPI` | 官方主控 | 主控地址。 |
| `ENABLE_NGINX` | `false` | 用 nginx 接管公网端口。 |
| `ENABLE_UPNP` | `false` | 用 UPnP 映射端口。 |
| `NO_DAEMON` | `false` | 单进程运行。 |
| `LOGLEVEL` | `info` | 日志级别。 |
| `LOG_DIR` | – | 日志按类型落盘到该目录。 |

完整列表、YAML 规则和各后端的选项见 [docs/CONFIG.md](docs/CONFIG.md)。

## 存储后端

| 取值 | 后端 |
| --- | --- |
| `file` | 本地磁盘（默认），对象落在 `cache/<ab>/<hash>` |
| `minio` | S3 兼容对象存储，自实现 SigV4 |
| `oss` | 阿里云 OSS，自实现 Signature V1 |
| `webdav` | 通用 WebDAV |
| `alist` | AList / OpenList，带签名直链缓存 |

`storage` 也可以写成 `sources` 列表做多源：读请求轮转，写入复制到每一份。

## HTTP 接口

| 路由 | 用途 |
| --- | --- |
| `GET /download/{hash}` | 返回对象，需要 `s`/`e` 签名，未命中时回源主控。 |
| `GET /measure/{size}` | 带宽探针，上限 200 MiB。 |
| `GET /auth` | nginx `auth_request` 用的内部签名校验端点。 |

## 文档

- [docs/CONFIG.md](docs/CONFIG.md) —— 配置参考。
- [docs/PORTING.md](docs/PORTING.md) —— 与 Node 版的模块映射和差异。
- [CLAUDE.md](CLAUDE.md) —— 代码规范。

## 许可证

Apache-2.0，见 [LICENSE](LICENSE) 与 [NOTICE](NOTICE)。

移植自 [bangbang93/openbmclapi](https://github.com/bangbang93/openbmclapi)（MIT），
其版权声明保留在 [NOTICE](NOTICE) 中。