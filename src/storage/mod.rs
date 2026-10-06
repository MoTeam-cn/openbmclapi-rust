//! Pluggable file storage backends.
//!
//! Every backend implements the same `Storage` trait; the download route only
//! talks to that trait. Mirrors `src/storage/*.ts` from the Node agent.

mod alist;
mod backend;
mod factory;
mod file;
pub mod oss;
mod path;
pub mod s3;
pub(crate) mod shared;
pub mod webdav;

pub use backend::{ServeRequest, ServeStat, Storage};
pub use factory::create;
pub(crate) use path::join_key;
