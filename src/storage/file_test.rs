use crate::storage::Storage;
use crate::testutil::TempDir;
use crate::types::{FileInfo, GcCounter};
use crate::util::hash_to_filename;

use super::FileStorage;

fn info(hash: &str, size: i64) -> FileInfo {
    FileInfo {
        path: format!("/download/{hash}"),
        hash: hash.to_string(),
        size,
        mtime: 0,
    }
}

const HASH: &str = "abcdef0123456789abcdef0123456789abcdef01";

#[tokio::test]
async fn a_written_object_is_readable_and_leaves_no_staging_file() {
    let dir = TempDir::new("file");
    let storage = FileStorage::new(dir.path().to_path_buf());
    let key = hash_to_filename(HASH);

    storage
        .write_file(&key, b"hello", &info(HASH, 5))
        .await
        .expect("the write succeeds");
    assert!(storage.exists(&key).await.expect("exists works"));

    let left: Vec<String> = std::fs::read_dir(dir.path().join("ab"))
        .expect("the shard directory exists")
        .map(|entry| {
            entry
                .expect("a readable entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        left.len(),
        1,
        "only the object survives, not its staging file"
    );
    assert!(!left[0].ends_with(".part"), "found a leftover staging file");
}

#[tokio::test]
async fn a_staging_file_is_not_an_object() {
    let dir = TempDir::new("file");
    let storage = FileStorage::new(dir.path().to_path_buf());
    let key = hash_to_filename(HASH);
    let shard = dir.path().join("ab");
    std::fs::create_dir_all(&shard).expect("the shard directory");

    std::fs::write(shard.join(format!("{HASH}.1.0.part")), b"half").expect("stage a partial write");

    assert!(
        !storage.exists(&key).await.expect("exists works"),
        "a half-written object must not be visible"
    );
}

#[tokio::test]
async fn gc_spares_the_reserved_measure_folder() {
    let dir = TempDir::new("file");
    let storage = FileStorage::new(dir.path().to_path_buf());
    let shard = dir.path().join("ab");
    let probes = dir.path().join("measure");
    std::fs::create_dir_all(&shard).expect("the shard directory");
    std::fs::create_dir_all(&probes).expect("the probe directory");

    let orphan = shard.join(HASH);
    let probe = probes.join("1m");
    std::fs::write(&orphan, b"gone").expect("seed an orphan");
    std::fs::write(&probe, b"probe").expect("seed a probe");

    let counter: GcCounter = storage.gc(&[]).await.expect("gc works");

    assert_eq!(counter.count, 1, "only the orphan is collected");
    assert!(!orphan.exists(), "the orphan is gone");
    assert!(probe.exists(), "a reserved probe must survive collection");
}

#[tokio::test]
async fn gc_removes_orphans_but_spares_a_write_in_flight() {
    let dir = TempDir::new("file");
    let storage = FileStorage::new(dir.path().to_path_buf());
    let shard = dir.path().join("ab");
    std::fs::create_dir_all(&shard).expect("the shard directory");

    let orphan = shard.join(HASH);
    let staging = shard.join(format!("{HASH}.9.9.part"));
    std::fs::write(&orphan, b"gone").expect("seed an orphan");
    std::fs::write(&staging, b"half").expect("seed a staging file");

    let counter: GcCounter = storage.gc(&[]).await.expect("gc works");

    assert_eq!(counter.count, 1, "only the orphan is collected");
    assert!(!orphan.exists(), "the orphan is gone");
    assert!(
        staging.exists(),
        "a write in flight must survive collection"
    );
}
