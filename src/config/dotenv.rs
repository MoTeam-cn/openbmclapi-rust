//! The `.env` reader.
//!
//! Hand-written rather than delegated to a crate: dotenvy strips the quotes out
//! of an unquoted value, so the Node agent's `CLUSTER_STORAGE_OPTIONS={"url":...}`
//! arrives as invalid JSON. The Node agent read that file with dotenv, which
//! keeps it intact, so a migration that depends on it has to as well.

use std::path::Path;

/// Load a `.env` file, letting the real environment win.
///
/// A variable already present in the process environment is left alone, which is
/// what dotenv does and what an operator expects when they export something to
/// override the file. A missing or unreadable file is not an error: the file is
/// optional.
pub fn load(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    apply(&text);
}

/// Apply an `.env` document to the process environment.
pub fn apply(text: &str) {
    for (key, value) in parse(text) {
        if std::env::var_os(&key).is_none() {
            std::env::set_var(key, value);
        }
    }
}

/// Split an `.env` document into pairs, dropping comments and blank lines.
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        pairs.push((key.to_string(), unquote(raw.trim())));
    }
    pairs
}

/// Strip the quotes around a value, and an inline comment from an unquoted one.
///
/// Only a value wrapped as a whole is unquoted, so the quotes inside a JSON
/// object survive.
fn unquote(raw: &str) -> String {
    let bytes = raw.as_bytes();
    if raw.len() >= 2 && bytes[0] == b'"' && bytes[raw.len() - 1] == b'"' {
        return raw[1..raw.len() - 1].to_string();
    }
    if raw.len() >= 2 && bytes[0] == b'\'' && bytes[raw.len() - 1] == b'\'' {
        return raw[1..raw.len() - 1].to_string();
    }
    match raw.find(" #") {
        Some(at) => raw[..at].trim_end().to_string(),
        None => raw.to_string(),
    }
}

#[cfg(test)]
#[path = "dotenv_test.rs"]
mod tests;
