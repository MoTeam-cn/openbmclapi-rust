//! Hand-written reader for the small YAML subset used by config.yaml.
//!
//! Supported:
//!
//! - comments: a # at the start of a line or after whitespace, never inside a
//!   quoted scalar
//! - block mappings (key: value), nested by indentation
//! - block sequences (- item), including (- key: value)
//! - flow sequences [a, b] and flow mappings {a: 1, b: 2}, nested to any depth
//! - scalars: plain, 'single quoted', "double quoted" (honouring backslash
//!   escapes for backslash, double quote, n, t, r and uXXXX)
//! - types: null / ~ / empty, true / false, integers, floats, everything else
//!   a string
//! - a key with nothing after the colon opens an empty mapping, so a nested
//!   block can follow
//!
//! Deliberately unsupported, each reported with its 1-based line: anchors and
//! aliases (& and *), tags (!), multi-line scalars (| and >), document markers
//! (--- and ...), merge keys (<<) and duplicate keys.

mod block;
mod error;
mod flow;
mod parser;
mod scalar;

pub(crate) use block::parse;
