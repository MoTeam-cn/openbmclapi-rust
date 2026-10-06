//! Command-line surface: `run` (the default), `init` and `migrate`.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::config;
use crate::error::{Error, Result};

/// Configuration file used when `--config` is not given.
pub const DEFAULT_CONFIG_FILE: &str = "config.yaml";

/// Environment file `migrate` looks for when it is given no argument.
pub const DEFAULT_ENV_FILE: &str = ".env";

/// bmclapi@home 集群节点。
#[derive(Debug, Parser)]
#[command(name = "openbmclapi", version, about = "OpenBMCLAPI 集群节点")]
pub struct Cli {
    /// 要加载的 YAML 配置文件。
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// 程序要做什么。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// 运行节点。不给子命令时就是这个。
    Run,
    /// 写出一份带注释的配置文件后退出。
    Init {
        /// 覆盖已存在的文件。
        #[arg(long)]
        force: bool,
    },
    /// 把 Node 版的 .env 转成本程序能读的配置。
    Migrate {
        /// 要转换的文件；缺省是当前目录下的 .env，`-` 表示从标准输入读取。
        #[arg(value_name = "FILE")]
        source: Option<PathBuf>,
        /// 覆盖已存在的文件。
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
            Some(path) => Err(Error::Config(format!("找不到配置文件：{}", path.display()))),
            None => {
                let default = Path::new(DEFAULT_CONFIG_FILE);
                Ok(default.exists().then_some(default))
            }
        }
    }

    /// Where `init` and `migrate` write, honouring an explicit `--config`.
    pub fn init_target(&self) -> PathBuf {
        self.config
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILE))
    }
}

/// Write the starter configuration, refusing to clobber without `--force`.
pub fn write_config(path: &Path, force: bool) -> Result<()> {
    write_new(path, config::init_template(), force)?;
    // Both writers run before the tracing subscriber exists, so this goes to stdout.
    println!("已写入 {}", path.display());
    Ok(())
}

/// Convert a Node `.env` into our configuration, refusing to clobber without `--force`.
pub fn write_migration(source: Option<&Path>, target: &Path, force: bool) -> Result<()> {
    let (text, label) = read_source(source)?;
    let yaml = config::convert(&text, &label)?;
    write_new(target, &yaml, force)?;
    println!("已写入 {}", target.display());
    Ok(())
}

/// Read the `.env` to convert: a named file, `-` for stdin, or the default path.
fn read_source(source: Option<&Path>) -> Result<(String, String)> {
    let path = match source {
        Some(path) if path.as_os_str() == "-" => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
            return Ok((text, "标准输入".to_string()));
        }
        Some(path) => path,
        None => Path::new(DEFAULT_ENV_FILE),
    };
    match std::fs::read_to_string(path) {
        Ok(text) => Ok((text, path.display().to_string())),
        Err(e) => Err(Error::Config(format!(
            "无法读取 {}：{e}。请把 Node 版的 .env 放在当前目录，或用参数指定文件，或从标准输入传入。",
            path.display()
        ))),
    }
}

/// Refuse to clobber, create the parent directory, then write.
fn write_new(path: &Path, body: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        return Err(Error::Config(format!(
            "{} 已存在，要覆盖请加 --force",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, body)?;
    Ok(())
}
