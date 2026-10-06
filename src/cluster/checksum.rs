//! Pure helpers for checksum verification and address classification.

use std::net::IpAddr;

use sha1::{Digest, Sha1};

/// Verify a downloaded buffer against the master checksum.
///
/// A 32 character hash is MD5, anything else is SHA-1 — matching `file.ts`.
pub fn validate_file(buffer: &[u8], checksum: &str) -> bool {
    if checksum.len() == 32 {
        format!("{:x}", md5::compute(buffer)) == checksum
    } else {
        let mut hasher = Sha1::new();
        hasher.update(buffer);
        hex::encode(hasher.finalize()) == checksum
    }
}

/// `ipaddr.range() === 'unicast'` equivalent for IPv4/IPv6.
pub fn is_unicast(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.octets()[0] >= 240)
        }
        IpAddr::V6(v6) => !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast()),
    }
}

#[cfg(test)]
#[path = "checksum_test.rs"]
mod tests;
