# Porting notes

The agent was ported from [`bangbang93/openbmclapi`](https://github.com/bangbang93/openbmclapi)
v1.14.0 (`06274c3`, `v1.14.0-65-g06274c3`). The Node source lives in
`openbmclapi/` in the working tree for reference and is excluded from this
repository.

## Module map

| Node source | Rust module | Notes |
| --- | --- | --- |
| `src/index.ts` | `src/main.rs`, `src/daemon.rs` | Worker supervision instead of `cluster.fork`; same backoff (factor 2, cap 60 s, ±20 % jitter, reset on ready). |
| `src/bootstrap.ts` | `src/bootstrap/` | Same ordering: token → connect → certificate → listen → port-check → storage check → sync → GC → enable. Every instance shares one storage backend, so that verification runs once. |
| `src/config.ts` | `src/config/` | Same `CLUSTER_*` variables and defaults; `.env` still loaded. Adds a YAML file and several node identities. |
| `src/token.ts` | `src/token.rs` | HMAC-SHA256 challenge/response; refresh at `max(ttl − 10 min, ttl/2)`. |
| `src/cluster.ts` | `src/cluster/`, `src/client.rs`, `src/routes/` | Split into orchestration, the master client and the route handlers. |
| `src/keepalive.ts` | `src/keepalive.rs` | 60 s reporting, 10 s ack timeout, restart after 3 failures. |
| `src/upnp.ts` | `src/upnp/` | SSDP + SOAP instead of `@xmcl/nat-api`; 30 min renewal, 1 h lease. |
| `src/util.ts` | `src/util.rs` | `hashToFilename`, `checkSign`, `getSize` including the range parser. |
| `src/file.ts` | `cluster::validate_file` | MD5 for 32-char hashes, SHA-1 otherwise. |
| `src/constants.ts` | `src/filelist.rs` | The avsc schema is replaced by a direct Avro binary decoder. |
| `src/logger.ts` | `src/logger/` | `tracing` + `EnvFilter` instead of pino; the line shapes are reproduced. |
| `src/routes/auth.route.ts` | `src/routes/auth.rs` | |
| `src/routes/measure.route.ts` | `src/routes/measure.rs` | |
| `src/storage/base.storage.ts` | `src/storage/backend.rs`, `src/storage/factory.rs` | `IStorage` → the `Storage` trait plus the factory. |
| `src/storage/file.storage.ts` | `src/storage/file.rs` | Adds single-range `206` responses. |
| `src/storage/minio.storage.ts` | `src/storage/s3/` | Hand-written SigV4 + presigning instead of the `minio` SDK. |
| `src/storage/oss.storage.ts` | `src/storage/oss/` | Hand-written Aliyun OSS Signature V1 instead of `ali-oss`. |
| `src/storage/webdav.storage.ts` | `src/storage/webdav/` | Raw PROPFIND/MKCOL/PUT/DELETE instead of the `webdav` package. |
| `src/storage/alist-webdav.storage.ts` | `src/storage/alist.rs` | Same redirect cache, persisted to `cache/redirectUrl.json`. |
| – | `src/storage/shared/` | XML tag/entity decoding and object-key percent-encoding shared by the S3 and OSS backends. |
| `src/modules/got-hooks.ts` | – | Not needed: the master client checks status codes explicitly. |
| `src/http2-express.d.ts` | – | Replaced by hyper's auto h1/h2 driver. |
| socket.io-client | `src/socketio/` | Hand-written engine.io v4 / socket.io v4 client over websockets. |

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
11. **Progress reporting.** The `cli-progress` multi-bar is reproduced: two
    lines redrawn in place, one over the file count and one over the bytes of
    the object on show. Concurrent downloads each keep their own byte counter
    and the bar shows the oldest one still in flight, so unrelated files never
    share a number. The display only draws on a terminal; a piped run, or
    `PLAIN_LOG`, falls back to the periodic `sync progress` line every 100 files.
12. **Upstream resilience.** The Node agent had no error boundary on the WebDAV
    and alist paths, so a struggling upstream (typically AList/OpenList) took
    the agent down with it. This port wraps every WebDAV request in a circuit
    breaker, an adaptive concurrency cap and bounded retries, streams proxied
    bodies instead of buffering them, treats a 429 as the WebDAV auth lockout
    rather than as load, and answers `503` plus `Retry-After` when the backend
    itself is the problem.
13. **A YAML configuration file.** Node read the environment only. This port
    layers `config.yaml` (or `--config FILE`) over the environment, so a file
    alone can configure an agent, and adds `openbmclapi init` to write a
    commented skeleton. The YAML reader is hand-written: the offline registry
    carries no YAML crate, and the project already hand-writes Avro, SigV4 and
    the engine.io client. It emits `serde_json::Value`, so every existing
    option lookup is untouched.
14. **Multi-source storage pools.** A `sources` list builds a pool that
    round-robins downloads across the sources, falls through to the next one
    when a source fails, replicates writes to every source, treats an object as
    present only when all sources have it, unions the missing-file reports and
    sums the GC counters. The local `file` backend is rejected inside a pool
    because it is the agent's cache rather than a remote mirror.
15. **Structured logs.** `LOG_FORMAT=json` emits one JSON object per line for a
    log collector; `PLAIN_LOG` still controls ANSI output and `RUST_LOG`
    overrides `LOGLEVEL` for per-module filtering.

16. **nginx is actually started.** The front-end was implemented but never
    invoked, so `ENABLE_NGINX=true` silently did nothing. The agent now binds a
    loopback port and hands it to nginx, which owns the public port and
    terminates TLS; `/auth` is reachable again as nginx's `auth_request` target.
17. **The local file backend writes atomically.** Node wrote straight to the
    final path, so a concurrent download could observe a half-written object.
    This port fills a `.part` sibling and renames it into place, and the GC
    skips staging files.
18. **The sync pass holds a byte budget.** Downloads are buffered whole so
    their checksum can be verified, and `concurrency` alone bounds how many run
    at once but not how large they are. A budget (`SYNC_MEMORY_BUDGET`, default
    256 MiB) is handed out by size, so a large object takes the budget alone.
19. **The sync pass checks free space first.** It refuses a pass that would not
    fit and warns once the filesystem drops below 5% free, instead of filling
    the disk and failing in less legible ways.

20. **Bandwidth probes live in the backend.** Node generated the `/measure`
    payload in process, so the probe never touched the storage backend and
    measured little more than the agent's own loopback. A remote backend now
    gets one object per configured size, uploaded after the storage check under
    a reserved `measure/` folder and named by the bare size (`measure/10`), so a
    probe travels the real backend-to-client path. `measure/0` is an empty
    placeholder because the master's liveness probe asks for it. The local
    `file` backend is deliberately not seeded: it is the disk the agent already
    runs on, so the route keeps generating the payload. The payload is an
    incompressible xorshift stream rather than a repeating pattern, so a
    compressing backend or CDN cannot flatter the measurement, and every
    backend's GC skips the reserved folder.
21. **Per-category log files.** `LOG_DIR` appends the stream to `access.log`,
    `sync.log`, `error.log` and `agent.log` alongside the console. Node wrote
    everything to stdout and left collection to the operator. Files are opened
    for append, so a restart continues the history and rotation stays with
    logrotate and friends.

22. **Log line shapes.** The application line is what pino-pretty printed with
    `translateTime: 'SYS:standard'` and `singleLine: true`: local time with its
    offset, the level, the pid, then the message with any structured fields
    trailing, and no module path. The access line is morgan's `combined`, which
    is Apache's common log format plus referrer and user agent; it needs the
    peer address, so a connection stamps it onto every request it carries.
    Levels are coloured only when the writer has a terminal.

23. **Several nodes in one process.** `instances` (or `CLUSTER_INSTANCES`) runs
    several identities at once over one shared storage backend, so the file
    verification is a single pass and the remaining nodes go straight to
    activation. Node ran one node per process, which meant one download pass
    per node against the same backend.

## Not ported

* `pkg`-based single-binary packaging (`package.json#pkg`) — `cargo build`
  already produces a self-contained executable.
* pino's pretty transport. Logging goes through `tracing`, with the two line
  shapes reproduced by hand; use `LOG_FORMAT=json` for a collector and
  `PLAIN_LOG` to control ANSI output.
* The Node agent's `.gitlab-ci.yml`, ESLint/Prettier/husky tooling and
  `docker-compose` files. A GitHub Actions workflow replaces the lint/test
  pipeline.