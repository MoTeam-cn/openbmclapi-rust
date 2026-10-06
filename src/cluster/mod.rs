//! Cluster orchestration: registration, file sync, downloads and lifecycle.
//!
//! Split by responsibility: the type and its lifecycle in `cluster`, master
//! RPCs in `master`, file sync in `sync`, accounting in `counters` and the
//! pure checksum helpers in `checksum`.

mod checksum;
// The prescribed split keeps the type in a same-named submodule; that is a
// deliberate layout choice, not accidental module inception.
#[allow(clippy::module_inception)]
mod cluster;
mod counters;
mod master;
mod sync;

pub use checksum::{is_unicast, validate_file};
pub use cluster::Cluster;
