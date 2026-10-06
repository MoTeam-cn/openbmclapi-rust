//! Storage key helpers.

use std::path::{Path, PathBuf};

/// Resolve a `/`-separated storage key against a local directory.
///
/// `.` and `..` segments are dropped so a crafted key cannot escape the cache
/// directory.
pub(crate) fn join_key(base: &Path, key: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    for segment in key
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
    {
        path.push(segment);
    }
    path
}

#[cfg(test)]
#[path = "path_test.rs"]
mod tests;
