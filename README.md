# openbmclapi-rust

OpenBMCLAPI（bmclapi@home）集群节点的 Rust 实现，对应 Node 版 v1.14.0。

## 构建

```bash
cargo build --release
```

## 运行

```bash
./openbmclapi init       # 生成 config.yaml，每一项都有注释
./openbmclapi migrate    # 把 Node 版的 .env 转成 config.yaml
./openbmclapi            # 启动
```

## 配置

最简的一份：

```yaml
cluster_id: "集群 ID"
cluster_secret: "集群密钥"
storage:
  type: file
```

也可以用环境变量或 `.env`，键名与 Node 版一致。完整键列表、各存储后端选项、
多实例写法见 [docs/CONFIG.md](docs/CONFIG.md)。

## 存储后端

`file`、`minio`、`oss`、`webdav`、`alist`。

可以配多个源，节点按顺序轮询并自动跳过故障源。

## 关于 nginx

节点**直接对外提供 HTTP / HTTPS**，不会在本地拉起一个 nginx 当前端。Node 版有
`ENABLE_NGINX` 是因为它要复用 nginx，这里已删除，也不打算加回来。

需要 443、TLS 或 CDN 回源时，用你自己的反向代理或 CDN 指过来就行。`GET /auth`
保留着，方便你的反代用 `auth_request` 复用节点的签名校验。

其余差异见 [docs/PORTING.md](docs/PORTING.md)。

## 许可证

Apache-2.0，见 [LICENSE](LICENSE)。移植自
[bangbang93/openbmclapi](https://github.com/bangbang93/openbmclapi)（MIT），
上游声明保留在 [NOTICE](NOTICE)。
