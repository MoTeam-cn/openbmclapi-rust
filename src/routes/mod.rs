//! HTTP surface of the agent: `/auth`, `/download/{hash}` and `/measure/{size}`.

mod auth;
mod download;
mod measure;
mod router;

pub use router::router;
