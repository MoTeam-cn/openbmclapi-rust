//! Object-key joining, prefix stripping and percent-encoding for object stores.

use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};

/// Characters left unescaped by AWS `UriEncode`; everything else is encoded.
const AWS_UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// [`AWS_UNRESERVED`] plus `/`, so object paths keep their separators.
const AWS_UNRESERVED_PATH: &AsciiSet = &AWS_UNRESERVED.remove(b'/');

/// `encodeURIComponent` semantics, used for `content-disposition` filenames.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// `path.join(prefix, key)` with `/` separators.
pub(crate) fn join_object_key(prefix: &str, key: &str) -> String {
    let prefix = prefix.trim_matches('/');
    let key = key.trim_start_matches('/');
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}/{key}")
    }
}

/// Strip a configured prefix from an object key.
pub(crate) fn strip_prefix_key<'a>(key: &'a str, prefix: &str) -> &'a str {
    let prefix = prefix.trim_matches('/');
    if prefix.is_empty() {
        return key;
    }
    key.strip_prefix(prefix)
        .map(|rest| rest.trim_start_matches('/'))
        .unwrap_or(key)
}

/// Percent-encode an object key, preserving `/`.
pub(crate) fn encode_path(key: &str) -> String {
    percent_encode(key.as_bytes(), AWS_UNRESERVED_PATH).to_string()
}

/// Percent-encode a single query component.
pub(crate) fn encode_component(value: &str) -> String {
    percent_encode(value.as_bytes(), AWS_UNRESERVED).to_string()
}

/// Percent-encode a filename the way `encodeURIComponent` does.
pub(crate) fn encode_filename(name: &str) -> String {
    percent_encode(name.as_bytes(), URI_COMPONENT).to_string()
}

#[cfg(test)]
#[path = "keys_test.rs"]
mod tests;
