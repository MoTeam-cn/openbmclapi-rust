use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;

use crate::error::{Error, Result};
use crate::types::{FileInfo, GcCounter};

use super::MultiStorage;
use crate::storage::backend::{ServeRequest, ServeStat, Storage};

/// A backend that counts calls and can be told to fail or to hold nothing.
struct Fake {
    served: AtomicUsize,
    writes: AtomicUsize,
    fail: bool,
    has: bool,
}

impl Fake {
    fn new(fail: bool, has: bool) -> Arc<Fake> {
        Arc::new(Fake {
            served: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
            fail,
            has,
        })
    }
}

#[async_trait]
impl Storage for Fake {
    async fn check(&self) -> Result<bool> {
        Ok(!self.fail)
    }

    async fn write_file(&self, _path: &str, _content: &[u8], _info: &FileInfo) -> Result<()> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        if self.fail {
            return Err(Error::storage("write rejected"));
        }
        Ok(())
    }

    async fn exists(&self, _path: &str) -> Result<bool> {
        Ok(self.has)
    }

    async fn get_missing_files(&self, files: &[FileInfo]) -> Result<Vec<FileInfo>> {
        Ok(if self.has { Vec::new() } else { files.to_vec() })
    }

    async fn gc(&self, _files: &[FileInfo]) -> Result<GcCounter> {
        Ok(GcCounter { count: 1, size: 2 })
    }

    async fn serve(&self, _req: ServeRequest<'_>) -> Result<(Response, ServeStat)> {
        self.served.fetch_add(1, Ordering::Relaxed);
        if self.fail {
            return Err(Error::UpstreamUnavailable { retry_in_ms: 0 });
        }
        let response = Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .expect("a status-only response always builds");
        Ok((response, ServeStat { bytes: 1, hits: 1 }))
    }
}

fn request<'a>() -> ServeRequest<'a> {
    ServeRequest {
        hash: "abcdef",
        hash_path: "ab/abcdef",
        range: None,
        name: None,
    }
}

fn file(path: &str) -> FileInfo {
    FileInfo {
        path: path.to_string(),
        hash: "abcdef".to_string(),
        size: 1,
        mtime: 0,
    }
}

#[test]
fn refuses_an_empty_pool() {
    let error = MultiStorage::new(Vec::<Arc<dyn Storage>>::new());
    assert!(error.is_err(), "a pool needs at least one source");
}

#[test]
fn accepts_a_pool_of_one() {
    let pool = MultiStorage::new(vec![Fake::new(false, true) as Arc<dyn Storage>]);
    assert!(pool.is_ok(), "one source is a pool of one");
}

#[test]
fn the_probe_policy_needs_one_entry_per_source() {
    let error = MultiStorage::with_measure_redirect(
        vec![Fake::new(false, true) as Arc<dyn Storage>],
        vec![true, false],
    );
    assert!(
        error.is_err(),
        "a mismatched policy is a configuration error"
    );
}

#[tokio::test]
async fn a_probe_skips_the_sources_that_opted_out() {
    let quiet = Fake::new(false, true);
    let loud = Fake::new(false, true);
    let pool = MultiStorage::with_measure_redirect(
        vec![
            Arc::clone(&quiet) as Arc<dyn Storage>,
            Arc::clone(&loud) as Arc<dyn Storage>,
        ],
        vec![false, true],
    )
    .expect("the policy matches the sources");

    assert!(pool.measure_redirect(), "one source agreed");
    let served = pool.serve_measure(1).await.expect("the probe is served");
    assert!(served.is_some(), "the source that opted in answers");
    assert_eq!(
        quiet.served.load(Ordering::Relaxed),
        0,
        "this one opted out"
    );
    assert_eq!(loud.served.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn a_probe_reports_nothing_when_every_source_opted_out() {
    let pool = MultiStorage::with_measure_redirect(
        vec![Fake::new(false, true) as Arc<dyn Storage>],
        vec![false],
    )
    .expect("one source, one policy entry");

    assert!(!pool.measure_redirect(), "nobody agreed to be measured");
    let served = pool.serve_measure(1).await.expect("no error");
    assert!(served.is_none(), "the caller generates the payload instead");
}

#[tokio::test]
async fn a_probe_falls_through_a_source_that_holds_nothing() {
    let empty = Fake::new(false, false);
    let full = Fake::new(false, true);
    let pool = MultiStorage::new(vec![
        Arc::clone(&empty) as Arc<dyn Storage>,
        Arc::clone(&full) as Arc<dyn Storage>,
    ])
    .expect("two sources build a pool");

    let served = pool.serve_measure(1).await.expect("no error");
    assert!(served.is_some(), "the source holding the object answers");
}

#[tokio::test]
async fn rotates_downloads_across_the_sources() {
    let first = Fake::new(false, true);
    let second = Fake::new(false, true);
    let pool = MultiStorage::new(vec![
        Arc::clone(&first) as Arc<dyn Storage>,
        Arc::clone(&second) as Arc<dyn Storage>,
    ])
    .expect("two sources build a pool");

    for _ in 0..4 {
        pool.serve(request())
            .await
            .expect("a healthy source answers");
    }
    assert_eq!(first.served.load(Ordering::Relaxed), 2);
    assert_eq!(second.served.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn falls_through_when_a_source_fails() {
    let broken = Fake::new(true, true);
    let healthy = Fake::new(false, true);
    let pool = MultiStorage::new(vec![
        Arc::clone(&broken) as Arc<dyn Storage>,
        Arc::clone(&healthy) as Arc<dyn Storage>,
    ])
    .expect("two sources build a pool");

    let served = pool.serve(request()).await;
    assert!(served.is_ok(), "the healthy source must answer");
    assert!(broken.served.load(Ordering::Relaxed) >= 1);
}

#[tokio::test]
async fn reports_the_union_of_what_each_source_is_missing() {
    let complete = Fake::new(false, true);
    let empty = Fake::new(false, false);
    let pool = MultiStorage::new(vec![
        Arc::clone(&complete) as Arc<dyn Storage>,
        Arc::clone(&empty) as Arc<dyn Storage>,
    ])
    .expect("two sources build a pool");

    let files = vec![file("ab/one"), file("cd/two")];
    let missing = pool.get_missing_files(&files).await.expect("listing works");
    assert_eq!(missing.len(), 2, "anything one source lacks is missing");
}

#[tokio::test]
async fn exists_needs_every_source_to_have_the_object() {
    let pool = MultiStorage::new(vec![
        Fake::new(false, true) as Arc<dyn Storage>,
        Fake::new(false, false) as Arc<dyn Storage>,
    ])
    .expect("two sources build a pool");

    assert!(!pool.exists("ab/one").await.expect("exists works"));
}

#[tokio::test]
async fn replicates_writes_to_every_source() {
    let first = Fake::new(false, true);
    let second = Fake::new(false, true);
    let pool = MultiStorage::new(vec![
        Arc::clone(&first) as Arc<dyn Storage>,
        Arc::clone(&second) as Arc<dyn Storage>,
    ])
    .expect("two sources build a pool");

    pool.write_file("ab/one", b"body", &file("ab/one"))
        .await
        .expect("both sources accept the write");
    assert_eq!(first.writes.load(Ordering::Relaxed), 1);
    assert_eq!(second.writes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn tolerates_one_source_rejecting_a_write() {
    let broken = Fake::new(true, true);
    let healthy = Fake::new(false, true);
    let pool = MultiStorage::new(vec![
        Arc::clone(&broken) as Arc<dyn Storage>,
        Arc::clone(&healthy) as Arc<dyn Storage>,
    ])
    .expect("two sources build a pool");

    assert!(
        pool.write_file("ab/one", b"body", &file("ab/one"))
            .await
            .is_ok(),
        "one surviving replica is enough; the next sync pass repairs the rest"
    );
}

#[tokio::test]
async fn sums_gc_counters_across_the_sources() {
    let pool = MultiStorage::new(vec![
        Fake::new(false, true) as Arc<dyn Storage>,
        Fake::new(false, true) as Arc<dyn Storage>,
    ])
    .expect("two sources build a pool");

    let counter = pool.gc(&[]).await.expect("gc works");
    assert_eq!(counter.count, 2);
    assert_eq!(counter.size, 4);
}
