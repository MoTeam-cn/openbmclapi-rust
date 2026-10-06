//! Command-line surface: `run` (the default) and `init`.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::config;
use crate::error::{Error, Result};

/// Configuration file used when `--config` is not given.
pub const DEFAULT_CONFIG_FILE: &str = "config.yaml";

/// bmclapi@home cluster agent.
#[derive(Debug, Parser)]
#[command(name = "openbmclapi", version, about = "OpenBMCLAPI cluster agent")]
pub struct Cli {
    /// YAML configuration file to load.
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// What the process should do.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the agent. This is the default when no subcommand is given.
    Run,
    /// Write a commented configuration file and exit.
    Init {
        /// Overwrite the file if it already exists.
        #[arg(long)]
        force: bool,
    },
}

impl Cli {
    /// Parse the process arguments.
    pub fn parse_args() -> Self {
        Cli::parse()
    }

    /// Resolve the configuration file to load.
    ///
    /// A path named on the command line must exist. The default path is used
    /// only when it is actually there, so an agent with no config file keeps
    /// working from the environment alone.
    pub fn config_path(&self) -> Result<Option<&Path>> {
        match &self.config {
            Some(path) if path.exists() => Ok(Some(path.as_path())),
            Some(path) => Err(Error::Config(format!(
                "configuration file not found: {}",
                path.display()
            ))),
            None => {
                let default = Path::new(DEFAULT_CONFIG_FILE);
                Ok(default.exists().then_some(default))
            }
        }
    }

    /// Where `init` writes, honouring an explicit `--config`.
    pub fn init_target(&self) -> PathBuf {
        self.config
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILE))
    }
}

/// Write the starter configuration, refusing to clobber without `--force`.
pub fn write_config(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        return Err(Error::Config(format!(
            "{} already exists; pass --force to overwrite",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, config::init_template())?;
    // `init` runs before the tracing subscriber exists, so this one goes to stdout.
    println!("wrote {}", path.display());
    Ok(())
}
