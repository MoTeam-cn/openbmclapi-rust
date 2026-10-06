# openbmclapi-rust

OpenBMCLAPI（bmclapi@home）集群节点的 Rust 实现，对应 Node 版 v1.14.0。

节点做四件事：向主控注册、把主控的文件清单同步到存储后端、对外提供下载与测速、
定时回报流量。

## 构建

```bash
cargo build --release
```

## 运行

```bash
./openbmclapi init     # 生成 config.yaml，每一项都有注释
./openbmclapi          # 启动
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

## 文档

- [docs/CONFIG.md](docs/CONFIG.md) — 配置参考
- [docs/PORTING.md](docs/PORTING.md) — 与 Node 版的差异

## 许可证

Apache-2.0，见 [LICENSE](LICENSE)。移植自
[bangbang93/openbmclapi](https://github.com/bangbang93/openbmclapi)（MIT），
上游声明保留在 [NOTICE](NOTICE)。