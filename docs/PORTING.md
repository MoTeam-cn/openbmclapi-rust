# Porting notes

The agent was ported from [`bangbang93/openbmclapi`](https://github.com/bangbang93/openbmclapi)
v1.14.0 (`06274c3`, `v1.14.0-65-g06274c3`). The Node source lives in
`openbmclapi/` in the working tree for reference and is excluded from this
repository.

## Module map

| Node source | Rust module | Notes |
| --- | --- | --- |
| `src/index.ts` | `src/main.rs` | Worker supervision instead of `cluster.fork`; same backoff (factor 2, cap 60 s, ±20 % jitter, reset on ready). |
| `src/bootstrap.ts` | `src/bootstrap.rs` | Same ordering: token → connect → certificate → listen → port-check → storage check → sync → GC → enable. |
| `src/config.ts` | `src/config.rs` | Same `CLUSTER_*` variables and defaults; `.env` still loaded. |
| `src/token.ts` | `src/token.rs` | HMAC-SHA256 challenge/response; refresh at `max(ttl − 10 min, ttl/2)`. |
| `src/cluster.ts` | `src/cluster.rs`, `src/client.rs`, `src/routes/*` | Split into orchestration, the master client and the route handlers. |
| `src/keepalive.ts` | `src/keepalive.rs` | 60 s reporting, 10 s ack timeout, restart after 3 failures. |
| `src/upnp.ts` | `src/upnp.rs` | SSDP + SOAP instead of `@xmcl/nat-api`; 30 min renewal, 1 h lease. |
| `src/util.ts` | `src/util.rs` | `hashToFilename`, `checkSign`, `getSize` including the range parser. |
| `src/file.ts` | `cluster::validate_file` | MD5 for 32-char hashes, SHA-1 otherwise. |
| `src/constants.ts` | `src/filelist.rs` | The avsc schema is replaced by a direct Avro binary decoder. |
| `src/logger.ts` | `src/logger.rs` | `tracing` + `EnvFilter` instead of pino. |
| `src/routes/auth.route.ts` | `src/routes/auth.rs` | |
| `src/routes/measure.route.ts` | `src/routes/measure.rs` | |
| `src/storage/base.storage.ts` | `src/storage/mod.rs` | `IStorage` → the `Storage` trait plus the factory. |
| `src/storage/file.storage.ts` | `src/storage/file.rs` | Adds single-range `206` responses. |
| `src/storage/minio.storage.ts` | `src/storage/s3.rs` | Hand-written SigV4 + presigning instead of the `minio` SDK. |
| `src/storage/oss.storage.ts` | `src/storage/oss.rs` | Hand-written Aliyun OSS Signature V1 instead of `ali-oss`. |
| `src/storage/webdav.storage.ts` | `src/storage/webdav.rs` | Raw PROPFIND/MKCOL/PUT/DELETE instead of the `webdav` package. |
| `src/storage/alist-webdav.storage.ts` | `src/storage/alist.rs` | Same redirect cache, persisted to `cache/redirectUrl.json`. |
| `src/modules/got-hooks.ts` | – | Not needed: the master client checks status codes explicitly. |
| `src/http2-express.d.ts` | – | Replaced by hyper's auto h1/h2 driver. |
| socket.io-client | `src/socketio.rs` | Hand-written engine.io v4 / socket.io v4 client over websockets. |

## Behavioural deviations

These are intentional and each one is a deliberate choice, not an oversight.

1. **Runtime flavour.** `flavor.runtime` is reported as `Rust/<crate version>`
   where Node reported `Node.js/<process.version>`. The field is informational.
2. **Storage keys use `/` on every platform.** Node's `path.join` produced
   backslash-separated object keys on Windows; here remote keys are always
   `ab/abcdef…`, and only the local file backend maps them onto native paths.
3. **nginx upstream is TCP, not a unix socket.** `ENABLE_NGINX` binds the agent
   to an ephemeral loopback port and renders that into the nginx config, so the
   feature works on Windows too. The template also drops the `user` directive
   (unsupported by nginx for Windows) and uses `http2 on;` (nginx ≥ 1.25.1).
4. **MinIO garbage collection compares basenames.** Node compared the full
   relative key against a set of hashes, which classified every object as
   expired. Both cloud backends now compare the object basename (the content
   hash) so a GC pass cannot delete live data.
5. **S3 uses path-style addressing.** `minio-js` may pick virtual-host style for
   non-IP hosts. Path-style is used unconditionally because the configuration
   schema exposes no `pathStyle` switch and it is the safest choice for MinIO
   and other S3-compatible endpoints.
6. **OSS uses a read timeout rather than a total timeout** in proxy mode, so a
   long streaming download is not cut off at 300 s. The MinIO client keeps the
   total timeout.
7. **Token refresh retries.** Node scheduled the next refresh only after a
   successful refresh; this port retries on the same schedule after a failure.
8. **File-list refresh uses the new list.** Node passed the *initial* file list
   to `syncFiles` inside its periodic check; here the freshly fetched list is
   used, which is what the surrounding code clearly intends.
9. **Multi-range requests.** The local file backend answers a single `Range` with
   `206`; multi-range requests receive the full `200` body (a valid server
   response) instead of a `multipart/byteranges` payload.
10. **Redirect reporting is coarser.** `reqwest` exposes the final URL but not
    the intermediate chain, so `openbmclapi/report` receives
    `[requested, final]` rather than every hop.
11. **Progress reporting.** The `cli-progress` multi-bar is replaced by a
    periodic `sync progress` log line every 100 files.

## Not ported

* `pkg`-based single-binary packaging (`package.json#pkg`) — `cargo build`
  already produces a self-contained executable.
* pino's pretty transport. `PLAIN_LOG` controls ANSI output; use
  `LOGLEVEL=debug` plus an external collector for structured logs.
* The Node agent's `.gitlab-ci.yml`, ESLint/Prettier/husky tooling and
  `docker-compose` files. A GitHub Actions workflow replaces the lint/test
  pipeline.
