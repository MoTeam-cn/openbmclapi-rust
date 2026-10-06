//! End-to-end checks for the agent HTTP surface.
//!
//! Boots the real router against the local file storage backend and exercises
//! the signature-protected download path, the bandwidth probe and the auth
//! endpoint.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use openbmclapi::cluster::Cluster;
use openbmclapi::config::Config;
use openbmclapi::server::HttpServer;
use openbmclapi::types::FileInfo;
use openbmclapi::util::{hash_to_filename, now_ms};
use sha1::{Digest, Sha1};

const SECRET: &str = "integration-secret";

/// Sign `secret || hash || e`, where `e` is the base36 deadline as it appears
/// in the query string (exactly what `util::check_sign` verifies).
fn sign(hash: &str, secret: &str, expires: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(secret.as_bytes());
    hasher.update(hash.as_bytes());
    hasher.update(expires.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn base36(mut value: i64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".into();
    }
    let mut buffer = Vec::new();
    while value > 0 {
        buffer.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    buffer.reverse();
    String::from_utf8(buffer).unwrap()
}

fn signed_query(hash: &str) -> String {
    let expires = base36(now_ms() + 60_000);
    format!("s={}&e={}", sign(hash, SECRET, &expires), expires)
}

/// Send a request, retrying once when the transport itself fails.
///
/// Each run binds a fresh loopback port, and Windows can abort a pooled
/// keep-alive connection with WSAECONNABORTED when the client writes to it at
/// the wrong moment. These are idempotent GETs, so one retry makes the suite
/// deterministic; a genuine server-side fault still fails both attempts.
async fn send<F>(url: &str, build: F) -> reqwest::Response
where
    F: Fn() -> reqwest::RequestBuilder,
{
    for attempt in 1..=2 {
        match build().send().await {
            Ok(response) => return response,
            Err(e) if attempt == 1 && (e.is_connect() || e.is_request() || e.is_timeout()) => {
                eprintln!("transport error on {url}, retrying once: {e}");
            }
            Err(e) => panic!("GET {url} failed: {e}"),
        }
    }
    unreachable!("the loop returns or panics")
}

#[tokio::test(flavor = "multi_thread")]
async fn download_measure_and_auth() {
    // The file backend derives its cache directory from the working directory,
    // so the test moves into a scratch tree. Windows refuses to delete the
    // process's current directory, so the original is kept to step back into.
    let start_dir = std::env::current_dir().expect("the working directory");
    let workdir = std::env::temp_dir().join(format!("openbmclapi-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workdir);
    std::fs::create_dir_all(&workdir).unwrap();
    std::env::set_current_dir(&workdir).unwrap();

    std::env::set_var("CLUSTER_ID", "integration-cluster");
    std::env::set_var("CLUSTER_SECRET", SECRET);
    std::env::set_var("CLUSTER_STORAGE", "file");
    std::env::set_var("DISABLE_ACCESS_LOG", "false");
    std::env::remove_var("CLUSTER_STORAGE_OPTIONS");

    // The access log is left on so the request middleware and the combined
    // formatter are exercised together; try_init is ignored if a subscriber is
    // already installed.
    openbmclapi::logger::init("info", true, openbmclapi::logger::LogFormat::Pretty, None);
    let config = Config::from_env().expect("config");
    let cluster = Cluster::new(config).expect("cluster");

    // Seed the cache with one object.
    let hash = "abcdef0123456789abcdef0123456789abcdef01";
    let payload = b"hello openbmclapi".to_vec();
    let info = FileInfo {
        path: "/download/seed".into(),
        hash: hash.to_string(),
        size: payload.len() as i64,
        mtime: now_ms(),
    };
    cluster
        .storage
        .write_file(&hash_to_filename(hash), &payload, &info)
        .await
        .expect("seed write");

    let router = openbmclapi::routes::router(Arc::clone(&cluster));
    let server = Arc::new(
        HttpServer::bind("127.0.0.1:0", router, None)
            .await
            .expect("bind"),
    );
    let addr = server.local_addr().expect("addr");
    let serving = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.serve().await })
    };
    let base = format!("http://{addr}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    // 1. Valid signature serves the object with the bmclapi headers.
    let url = format!("{base}/download/{hash}?{}", signed_query(hash));
    let response = send(&url, || client.get(&url)).await;
    assert_eq!(response.status(), 200, "signed download must succeed");
    assert_eq!(
        response.headers().get("x-bmclapi-hash").unwrap(),
        hash,
        "hash header"
    );
    assert_eq!(
        response.headers().get("x-bmclapi-id").unwrap(),
        "integration-cluster"
    );
    let body = response.bytes().await.unwrap();
    assert_eq!(body.as_ref(), payload.as_slice());

    // 2. Invalid signature is rejected.
    let url = format!("{base}/download/{hash}?s=nope&e=zzz");
    let response = send(&url, || client.get(&url)).await;
    assert_eq!(response.status(), 403, "bad sign must be rejected");

    // 3. Unknown but well-formed hash is a 404.
    let other = "0000000000000000000000000000000000000000";
    let url = format!("{base}/download/{other}?{}", signed_query(other));
    let response = send(&url, || client.get(&url)).await;
    assert_eq!(response.status(), 404, "missing object must be a 404");

    // 4. The measure endpoint returns the requested megabyte count.
    let url = format!("{base}/measure/2?{}", signed_query("/measure/2"));
    let response = send(&url, || client.get(&url)).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-length").unwrap(),
        &(2 * 1024 * 1024).to_string()
    );
    assert_eq!(response.bytes().await.unwrap().len(), 2 * 1024 * 1024);

    // 5. Oversized measure requests are refused.
    let url = format!("{base}/measure/500?{}", signed_query("/measure/500"));
    let response = send(&url, || client.get(&url)).await;
    assert_eq!(response.status(), 400);

    // 6. The auth endpoint validates x-original-uri.
    let expires = base36(now_ms() + 60_000);
    let signature = sign(hash, SECRET, &expires);
    let original = format!("/download/{hash}?s={signature}&e={expires}");
    let url = format!("{base}/auth");
    let response = send(&url, || {
        client.get(&url).header("x-original-uri", &original)
    })
    .await;
    assert_eq!(response.status(), 204, "valid auth_request must be 204");

    let bad = format!("/download/{hash}?s=bad&e=bad");
    let response = send(&url, || client.get(&url).header("x-original-uri", &bad)).await;
    assert_eq!(response.status(), 403, "invalid auth_request must be 403");

    // 7. Range requests are honoured.
    let url = format!("{base}/download/{hash}?{}", signed_query(hash));
    let response = send(&url, || client.get(&url).header("range", "bytes=0-4")).await;
    assert_eq!(response.status(), 206);
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"hello");

    server.close();
    let _ = serving.await;
    // `close` stops the accept loop but does not await the connection tasks,
    // and Windows refuses to remove a tree that is still a working directory or
    // has an open handle in it. Retry briefly, then fail loudly: a silent
    // `let _ =` is exactly how scratch directories used to pile up.
    let _ = std::env::set_current_dir(&start_dir);
    let mut failure = None;
    for _ in 0..20 {
        match std::fs::remove_dir_all(&workdir) {
            Ok(()) => {
                failure = None;
                break;
            }
            Err(e) => {
                failure = Some(e);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    if let Some(e) = failure {
        panic!("the scratch directory outlived the test: {e}");
    }
}
