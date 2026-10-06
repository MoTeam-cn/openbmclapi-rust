//! WebDAV storage backend.
//!
//! Speaks plain WebDAV (PROPFIND / MKCOL / PUT / DELETE) over `reqwest`, so it
//! also backs the AList variant. Certificate validation is disabled to match
//! the Node agent, which passes `rejectUnauthorized: false`.

mod cache;
mod client;
mod response;
mod storage;
mod xml;

pub use client::{join_url, WebdavClient};
pub use storage::WebdavStorage;
pub use xml::{parse_multistatus, DavEntry};

pub(crate) use response::{empty_ok, redirect};
