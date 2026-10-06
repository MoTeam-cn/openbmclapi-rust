//! Speed-test objects kept inside the storage backend.
//!
//! The bandwidth probe used to be generated in-process, so it measured the
//! agent's loopback and never touched the backend. These objects live under a
//! reserved `speedtest` folder in the storage root instead, so a probe travels
//! the real path: backend to agent to client, or straight to the backend when
//! the backend answers with a redirect.
//!
//! The folder is reserved: no garbage collection may touch it.

use axum::response::Response;
use tracing::{info, warn};

use crate::error::Result;
use crate::types::FileInfo;
use crate::util::now_ms;

use super::backend::{ServeRequest, ServeStat, Storage};

/// Folder reserved for the probes, inside the storage root.
pub const DIR: &str = "speedtest";

/// Whether a storage-relative path lies inside the reserved folder.
pub fn is_reserved(relative: &str) -> bool {
    relative == DIR || relative.starts_with("speedtest/")
}

/// Storage key of the probe for `size_mib` megabytes.
pub fn key(size_mib: u64) -> String {
    format!("{DIR}/{size_mib}m")
}

/// Deterministic, incompressible payload for one probe.
///
/// A repeating pattern compresses away in transit, so a backend or CDN that
/// applies gzip would report a throughput that has nothing to do with moving
/// real data. This is an xorshift stream seeded from the size: stable across
/// runs and across nodes, and still not worth compressing.
pub fn payload(size_mib: u64) -> Vec<u8> {
    let mut out = vec![0u8; (size_mib as usize).saturating_mul(1024 * 1024)];
    let mut state = 0x9e37_79b9_7f4a_7c15u64 ^ size_mib.wrapping_mul(0x2545_f491_4f6c_dd1d);
    if state == 0 {
        state = 0x9e37_79b9_7f4a_7c15;
    }
    for chunk in out.chunks_mut(8) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let value = state.wrapping_mul(0x2545_f491_4f6c_dd1d).to_le_bytes();
        let len = chunk.len();
        chunk.copy_from_slice(&value[..len]);
    }
    out
}

/// Upload every size the backend does not already have.
///
/// Returns how many objects were written. A failure is logged and skipped: a
/// backend hiccup while seeding probes must not stop the agent from starting.
pub async fn ensure(storage: &dyn Storage, sizes: &[u64]) -> Result<usize> {
    let mut written = 0;
    for &size in sizes {
        let object = key(size);
        match storage.exists(&object).await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                warn!(size_mib = size, error = %e, "cannot check the speed-test object");
                continue;
            }
        }
        let bytes = payload(size);
        let info = FileInfo {
            path: format!("/{object}"),
            hash: object.clone(),
            size: bytes.len() as i64,
            mtime: now_ms(),
        };
        match storage.write_file(&object, &bytes, &info).await {
            Ok(()) => {
                info!(size_mib = size, "uploaded a speed-test object");
                written += 1;
            }
            Err(e) => {
                warn!(size_mib = size, error = %e, "cannot upload the speed-test object");
            }
        }
    }
    Ok(written)
}

/// Serve the stored probe for `size_mib`, if the backend has one.
///
/// `None` means there is nothing stored for that size, and the caller should fall
/// back to generating the payload.
pub async fn serve(storage: &dyn Storage, size_mib: u64) -> Result<Option<(Response, ServeStat)>> {
    let object = key(size_mib);
    if !storage.exists(&object).await? {
        return Ok(None);
    }
    let request = ServeRequest {
        hash: &object,
        hash_path: &object,
        range: None,
        name: None,
    };
    storage.serve(request).await.map(Some)
}

#[cfg(test)]
#[path = "speedtest_test.rs"]
mod tests;
