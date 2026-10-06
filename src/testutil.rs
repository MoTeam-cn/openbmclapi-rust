//! Scratch paths for unit tests.
//!
//! Everything a run creates lives under one directory in the system temp
//! directory, named with the process id, a sequence number and a label, so two
//! tests running in parallel cannot collide — a shared fixed name means one test
//! deletes the directory out from under another.
//!
//! Cleanup happens in `Drop`, so a failing assertion still leaves the machine
//! tidy, and the last guard out removes the now-empty root.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Directory every test artifact lives in, so a whole run can be swept at once.
const ROOT: &str = "openbmclapi-tests";

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A directory removed when the guard is dropped.
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create an empty directory unique to this test.
    pub(crate) fn new(label: &str) -> Self {
        let path = unique(label);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory");
        TempDir { path }
    }

    /// The directory.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        sweep();
    }
}

/// A file removed when the guard is dropped.
pub(crate) struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// Write `body` to a file unique to this test. `label` may carry an
    /// extension, for example `config.yaml`.
    pub(crate) fn write(label: &str, body: &str) -> Self {
        let path = unique(label);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the scratch directory");
        }
        std::fs::write(&path, body).expect("a scratch file");
        TempFile { path }
    }

    /// The file.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        sweep();
    }
}

/// A path under the run's root, unique to the caller.
fn unique(label: &str) -> PathBuf {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join(ROOT)
        .join(format!("{}-{sequence}-{label}", std::process::id()))
}

/// Remove the root once the last artifact is gone. Fails harmlessly while
/// another test still holds one, which is exactly what is wanted.
fn sweep() {
    let _ = std::fs::remove_dir(std::env::temp_dir().join(ROOT));
}
