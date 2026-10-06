//! Free-space probing for the filesystem that holds the cache.
//!
//! No disk-statistics crate is available offline and the project avoids vendor
//! SDKs anyway, so this calls the platform primitive directly: `statvfs` on
//! unix and `GetDiskFreeSpaceExW` on Windows.

use std::path::Path;

use crate::error::{Error, Result};

/// Free and total bytes of one filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskUsage {
    /// Bytes the current user may still write, which is what a sync consumes.
    pub free: u64,
    /// Size of the whole filesystem.
    pub total: u64,
}

impl DiskUsage {
    /// Free bytes as a fraction of the total, in the range 0.0 to 1.0.
    pub fn free_ratio(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.free as f64 / self.total as f64
    }
}

/// Probe the filesystem holding `path`, which must already exist.
pub fn usage(path: &Path) -> Result<DiskUsage> {
    probe(path)
}

#[cfg(unix)]
// The statvfs fields are c_ulong / fsblkcnt_t: 64-bit on the targets we ship but
// narrower on some 32-bit ones, so the widening casts below are required there
// and no-ops here. clippy only sees the target it is compiled for.
#[allow(clippy::unnecessary_cast)]
fn probe(path: &Path) -> Result<DiskUsage> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let raw = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::Other("the path contains a NUL byte".into()))?;
    // SAFETY: the syscall only writes into the struct we own, and the path
    // stays a valid NUL-terminated buffer for the duration of the call.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(raw.as_ptr(), &mut stat) };
    if rc != 0 {
        return Err(Error::Other(format!(
            "cannot read free space for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    let block = stat.f_frsize as u64;
    Ok(DiskUsage {
        free: stat.f_bavail as u64 * block,
        total: stat.f_blocks as u64 * block,
    })
}

#[cfg(windows)]
fn probe(path: &Path) -> Result<DiskUsage> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: the out-parameters are locals we own, and the path stays a valid
    // NUL-terminated wide buffer for the duration of the call.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
    if ok == 0 {
        return Err(Error::Other(format!(
            "cannot read free space for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    // The third out-parameter counts bytes an administrator could reclaim; the
    // first is what this process may actually use, which is the one that counts.
    let _ = total_free;
    Ok(DiskUsage { free, total })
}

#[cfg(not(any(unix, windows)))]
fn probe(_path: &Path) -> Result<DiskUsage> {
    Err(Error::Other(
        "free-space probing is not implemented on this platform".into(),
    ))
}

#[cfg(test)]
#[path = "disk_test.rs"]
mod tests;