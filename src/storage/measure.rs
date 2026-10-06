//! Bandwidth-probe objects kept inside the storage backend.
//!
//! The probe used to be generated in-process, so it measured the agent's
//! loopback and never touched the backend. Remote backends get one object per
//! configured size instead, under a reserved `measure` folder, so a probe
//! travels the real path: the backend answers with its own direct link and the
//! client pulls from there.
//!
//! The local `file` backend is deliberately excluded: it is the same disk the
//! agent runs on, so generating the payload on the fly is both faster and
//! cheaper than storing a copy.
//!
//! The folder is reserved: no garbage collection may touch it.

use axum::response::Response;
use tracing::{info, warn};

use crate::error::Result;
use crate::types::FileInfo;
use crate::util::now_ms;

use super::backend::{ServeRequest, ServeStat, Storage};

/// Folder reserved for the probes, inside the storage root.
pub const DIR: &str = "measure";

/// Whether a storage-relative path lies inside the reserved folder.
pub fn is_reserved(relative: &str) -> bool {
    relative == DIR || relative.starts_with("measure/")
}

/// Storage key of the probe for `size_mib` megabytes.
///
/// The name is the bare number, matching what the master asks for.
pub fn key(size_mib: u64) -> String {
    format!("{DIR}/{size_mib}")
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
                warn!(size_mib = size, error = %e, "无法检查测速对象");
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
                info!(size_mib = size, "已上传测速对象");
                written += 1;
            }
            Err(e) => {
                warn!(size_mib = size, error = %e, "无法上传测速对象");
            }
        }
    }
    Ok(written)
}

/// Serve the stored probe for `size_mib` from one specific backend.
///
/// `None` means that backend holds nothing for that size, so the caller should
/// either try another source or generate the payload.
pub async fn serve_from(
    source: &dyn Storage,
    size_mib: u64,
) -> Result<Option<(Response, ServeStat)>> {
    let object = key(size_mib);
    if !source.exists(&object).await? {
        return Ok(None);
    }
    let request = ServeRequest {
        hash: &object,
        hash_path: &object,
        range: None,
        name: None,
    };
    source.serve(request).await.map(Some)
}

#[cfg(test)]
#[path = "measure_test.rs"]
mod tests;
