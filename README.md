# openbmclapi-rust

A Rust implementation of the **OpenBMCLAPI** cluster agent — the `bmclapi@home`
node software that serves Minecraft file mirrors on behalf of
[`openbmclapi.bangbang93.com`](https://github.com/bangbang93/openbmclapi).

This is a from-scratch port of the Node.js agent **v1.14.0**. The upstream
TypeScript source is kept in `openbmclapi/` purely as the reference for the port
and is **not** part of this repository (see `.gitignore`).

## What the agent does

1. Authenticates against the master with an HMAC-SHA256 challenge/response.
2. Downloads the master's file list (zstd-compressed Avro) and mirrors anything
   missing into its storage backend, verifying MD5/SHA-1 checksums.
3. Serves signed `/download/:hash` requests from that storage.
4. Reports served bytes to the master every minute over socket.io.
5. Garbage-collects objects that dropped out of the master file list.

## Build

```bash
cargo build --release
# target/release/openbmclapi
```

Requires Rust 1.82 or newer. TLS is pure-rustls: no OpenSSL and no system
certificate tooling is needed.

## Run

```bash
export CLUSTER_ID=your-cluster-id
export CLUSTER_SECRET=your-cluster-secret
./target/release/openbmclapi
```

By default the process forks a supervised worker and restarts it with
exponential backoff if it dies. Set `NO_DAEMON=1` to run a single foreground
process, which is what container deployments normally want.

A `.env` file in the working directory is loaded automatically.

### Configuration

| Variable | Default | Description |
| --- | --- | --- |
| `CLUSTER_ID` | **required** | Cluster id issued by the master. |
| `CLUSTER_SECRET` | **required** | Cluster secret used for HMAC and download signatures. |
| `CLUSTER_IP` | – | Address advertised to the master when it cannot be auto-detected. |
| `CLUSTER_PORT` | `4000` | Local listening port. |
| `CLUSTER_PUBLIC_PORT` | `CLUSTER_PORT` | Port reported to the master. |
| `CLUSTER_BYOC` | `false` | Bring your own certificate instead of requesting one. |
| `CLUSTER_STORAGE` | `file` | `file`, `minio`, `oss`, `webdav` or `alist`. |
| `CLUSTER_STORAGE_OPTIONS` | – | JSON options for the selected backend. |
| `CLUSTER_BMCLAPI` | `https://openbmclapi.bangbang93.com` | Master base URL. |
| `SSL_KEY` / `SSL_CERT` | – | PEM path or inline PEM content (BYOC only). |
| `ENABLE_NGINX` | `false` | Put nginx in front of the agent. |
| `ENABLE_UPNP` | `false` | Map the public port with UPnP IGD. |
| `DISABLE_ACCESS_LOG` | `false` | Turn off per-request logging. |
| `DISABLE_SIGN` | `false` | Skip `s`/`e` signature validation (trusted networks only). |
| `NO_DAEMON` | `false` | Run a single process instead of supervising a worker. |
| `NO_FAST_ENABLE` | `false` | Ask the master to skip its fast-enable path. |
| `LOGLEVEL` | `info` | `trace`, `debug`, `info`, `warn`, `error`. |
| `PLAIN_LOG` | `false` | Disable ANSI colouring. |

### Storage options

`file` takes no options; objects land in `./cache/<ab>/<hash>`.

```jsonc
// minio — the URL carries credentials, bucket, prefix and region
{ "url": "https://key:secret@minio.example.com:9000/bucket/prefix?region=us-east-1",
  "internalUrl": "http://minio.internal:9000/bucket/prefix?region=us-east-1" }

// oss (Aliyun)
{ "accessKeyId": "…", "accessKeySecret": "…", "bucket": "…", "prefix": "cache",
  "internal": false, "proxy": true, "region": "oss-cn-hangzhou", "cname": false }

// webdav
{ "url": "https://dav.example.com/remote.php/dav", "username": "u", "password": "p",
  "basePath": "/openbmclapi" }

// alist — WebDAV plus a signed-redirect cache
{ "url": "http://alist.local:5244/dav", "username": "u", "password": "p",
  "basePath": "/openbmclapi", "cacheTtl": "1h" }
```

## HTTP surface

| Route | Purpose |
| --- | --- |
| `GET /download/{hash}` | Serve a cached object. Requires a valid `s`/`e` signature unless `DISABLE_SIGN` is set. Misses are fetched from the master, de-duplicated across concurrent requests and checksum-verified. |
| `GET /measure/{size}` | Bandwidth probe: streams `size` MiB (max 200) of `0066ccff`. |
| `GET /auth` | nginx `auth_request` target; validates the `x-original-uri` signature. |

The listener speaks HTTP/2 and HTTP/1.1 over TLS (ALPN) or plain HTTP/1.1,
negotiated per connection by hyper's auto driver.

## Architecture

```
src/
  main.rs        binary entry (delegates to daemon::entry)
  daemon.rs      daemon supervisor: worker restart/backoff
  bootstrap.rs   worker startup: auth, certificate, listen, sync, enable
  config.rs      environment configuration
  token.rs       HMAC challenge/response + background token refresh
  client.rs      master HTTP client (bearer auth, response cache)
  filelist.rs    hand-rolled Avro decoder for the master file list
  cluster/       registration, sync, download, GC, counters
  keepalive.rs   one-minute reporting loop with restart-on-error
  socketio/      engine.io v4 / socket.io v4 client over websockets
  server.rs      hyper auto (h1/h2) listener + rustls setup
  routes/        /auth, /download/{hash}, /measure/{size}
  storage/       file, minio (S3 SigV4), oss (Aliyun V1), webdav, alist
                 shared/ holds the XML and key helpers both cloud backends use
  upnp/          SSDP discovery + SOAP port mapping
  nginx.rs       optional nginx front-end
  tls.rs         PEM parsing for rustls
```

## Tests

```bash
cargo test --offline --lib
cargo test --offline --test http_surface
```

Unit tests cover the Avro decoder, signature verification, range arithmetic,
multistatus parsing, template rendering, duration parsing and both cloud
signature implementations. `tests/http_surface.rs` boots the real router against
the file backend and exercises signed downloads, bad signatures, 404s, ranges,
the measure endpoint and the nginx auth endpoint.

## Conventions

This repository follows [CLAUDE.md](CLAUDE.md). The parts that bite:

- No source file may exceed 350 lines; 301–350 needs a written justification.
- `lib.rs` / `main.rs` / any `mod.rs` is an entry point only — module docs,
  `mod` declarations and re-exports. Logic belongs in a sibling file.
- Unit tests live in `<module>_test.rs` next to the module and are pulled in with
  `#[cfg(test)] #[path = "<module>_test.rs"] mod tests;`. Cross-module tests live in `tests/`.
- Reuse before writing: the same logic appearing twice must be extracted.
- Verification is scoped, never `--all-targets`: `cargo check --offline --lib`,
  `cargo clippy --offline --lib`, `cargo test --offline --lib <module>::`. The whole-crate
  pass runs once, at collection time.

## Differences from the Node agent

See [docs/PORTING.md](docs/PORTING.md) for the full mapping and every deliberate
deviation. The short version:

* The runtime is described to the master as `Rust/<version>` instead of
  `Node.js/<version>`.
* Storage object keys always use `/` as the separator, so remote keys are
  identical on every platform (the Node agent used the host separator).
* nginx proxies to a loopback TCP port instead of a unix socket, so the feature
  also works on Windows.
* Aliyun OSS and S3 are implemented directly (SigV4 / OSS V1) instead of through
  vendor SDKs.

## License

MIT — see [LICENSE](LICENSE). Ported from
[bangbang93/openbmclapi](https://github.com/bangbang93/openbmclapi), also MIT.
