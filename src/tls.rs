//! Minimal PEM parsing for the certificate/key pair handed to rustls.
//!
//! Avoids a dependency on a PEM crate: the agent only ever deals with the two
//! PEM files the master issues (or the operator supplies).

use base64::Engine;
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
};

use crate::error::{Error, Result};

fn pem_blocks(pem: &str, label: &str) -> Vec<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut blocks = Vec::new();
    let mut rest = pem;
    while let Some(start) = rest.find(&begin) {
        let after_begin = &rest[start + begin.len()..];
        let Some(stop) = after_begin.find(&end) else {
            break;
        };
        let body: String = after_begin[..stop]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(body) {
            blocks.push(bytes);
        }
        rest = &after_begin[stop + end.len()..];
    }
    blocks
}

/// Parse every `CERTIFICATE` block of a PEM document.
pub fn parse_certificates(pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    let blocks = pem_blocks(pem, "CERTIFICATE");
    if blocks.is_empty() {
        return Err(Error::Config("no CERTIFICATE block found in PEM".into()));
    }
    Ok(blocks.into_iter().map(CertificateDer::from).collect())
}

/// Parse a PKCS#8, PKCS#1 or SEC1 private key from a PEM document.
pub fn parse_private_key(pem: &str) -> Result<PrivateKeyDer<'static>> {
    if let Some(block) = pem_blocks(pem, "PRIVATE KEY").into_iter().next() {
        return Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(block)));
    }
    if let Some(block) = pem_blocks(pem, "RSA PRIVATE KEY").into_iter().next() {
        return Ok(PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(block)));
    }
    if let Some(block) = pem_blocks(pem, "EC PRIVATE KEY").into_iter().next() {
        return Ok(PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(block)));
    }
    Err(Error::Config("no private key block found in PEM".into()))
}

#[cfg(test)]
#[path = "tls_test.rs"]
mod tests;
