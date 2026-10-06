//! Worker bootstrap: register every configured instance, verify once, serve.

mod all;
mod instance;

pub use all::run;
pub use instance::{wait_for_shutdown, READY_MARKER};
