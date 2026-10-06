//! Binary entry point.

use std::process::ExitCode;

use openbmclapi::daemon;

fn main() -> ExitCode {
    daemon::entry()
}
