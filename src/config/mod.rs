//! Agent configuration: the process environment plus an optional YAML file.
//!
//! env owns the Config type and reads the CLUSTER_* variables, file layers a
//! YAML document on top of those values, init holds the skeleton written by
//! the init subcommand, and yaml is the hand-written reader. The crate-facing
//! names are re-exported here so callers keep using crate::config::...

mod env;
mod file;
pub(crate) mod init;
mod instances;
mod value;
mod yaml;

pub use env::{Config, Flavor, StorageSource, DEFAULT_BMCLAPI_BASE, DEFAULT_PORT};
pub use file::{load, DEFAULT_FILE};
pub(crate) use init::init_template;
pub use instances::Instance;
