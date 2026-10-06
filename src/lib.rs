//! OpenBMCLAPI cluster agent — Rust implementation.
//!
//! This crate is a from-scratch port of the Node.js `openbmclapi` agent
//! (https://github.com/bangbang93/openbmclapi) to Rust.

pub mod bootstrap;
pub mod client;
pub mod cluster;
pub mod config;
pub mod error;
pub mod filelist;
pub mod keepalive;
pub mod logger;
pub mod nginx;
pub mod routes;
pub mod server;
pub mod socketio;
pub mod storage;
pub mod tls;
pub mod token;
pub mod types;
pub mod upnp;
pub mod util;

pub use error::{Error, Result};

/// Agent version reported to the master.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
