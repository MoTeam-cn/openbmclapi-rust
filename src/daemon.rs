//! Daemon supervisor: keeps a worker process alive and dispatches the entry point.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{error, info, warn};

use crate::bootstrap::{self, READY_MARKER};
use crate::cli::{self, Cli, Command as CliCommand};
use crate::config::Config;
use crate::logger;

/// Environment variable marking a supervised child process.
pub const WORKER_ENV: &str = "OPENBMCLAPI_WORKER";

/// Exit code a worker uses for an error that restarting cannot fix.
///
/// The supervisor watches for it and stops: a bad configuration is the same bad
/// configuration on the next attempt, and looping only buries the cause.
pub const FATAL_EXIT_CODE: u8 = 2;

const BACKOFF_FACTOR: f64 = 2.0;
const BACKOFF_MAX: f64 = 60.0;
const BACKOFF_JITTER: f64 = 0.2;

/// Load configuration and either run a worker or supervise one.
pub fn entry() -> ExitCode {
    let args = Cli::parse_args();
    match &args.command {
        Some(CliCommand::Init { force }) => {
            return match cli::write_config(&args.init_target(), *force) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("无法写入配置文件：{e}");
                    ExitCode::FAILURE
                }
            };
        }
        Some(CliCommand::Migrate { source, force }) => {
            return match cli::write_migration(source.as_deref(), &args.init_target(), *force) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("转换失败：{e}");
                    ExitCode::FAILURE
                }
            };
        }
        _ => {}
    }

    // The file is optional and never overrides the real environment.
    crate::config::dotenv::load(Path::new(cli::DEFAULT_ENV_FILE));
    let config_path = match args.config_path() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("配置错误：{e}");
            return ExitCode::from(FATAL_EXIT_CODE);
        }
    };
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("配置错误：{e}");
            return ExitCode::from(FATAL_EXIT_CODE);
        }
    };
    let format = logger::LogFormat::parse(Some(&config.log_format));
    logger::init(
        &config.log_level,
        config.plain_log,
        format,
        config.log_dir.as_deref(),
    );

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("无法启动异步运行时：{e}");
            return ExitCode::FAILURE;
        }
    };

    if config.no_daemon || std::env::var(WORKER_ENV).is_ok() {
        return match runtime.block_on(bootstrap::run(config)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                error!(error = %e, "启动失败");
                if e.is_fatal() {
                    ExitCode::from(FATAL_EXIT_CODE)
                } else {
                    ExitCode::FAILURE
                }
            }
        };
    }
    runtime.block_on(supervise())
}

/// Keep a worker process alive, restarting it with exponential backoff.
async fn supervise() -> ExitCode {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            error!(error = %e, "无法定位当前可执行文件");
            return ExitCode::FAILURE;
        }
    };

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        bootstrap::wait_for_shutdown().await;
        let _ = shutdown_tx.send(true);
    });

    let mut backoff = 1.0_f64;
    loop {
        let mut child = match Command::new(&exe)
            .env(WORKER_ENV, "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                error!(error = %e, "无法拉起 worker 进程");
                return ExitCode::FAILURE;
            }
        };

        let stdout = child.stdout.take();
        let ready = std::sync::Arc::new(tokio::sync::Notify::new());
        if let Some(stdout) = stdout {
            let ready = std::sync::Arc::clone(&ready);
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.contains(READY_MARKER) {
                        ready.notify_one();
                    } else {
                        println!("{line}");
                    }
                }
            });
        }

        let outcome = loop {
            tokio::select! {
                status = child.wait() => break Some(status),
                _ = shutdown_rx.changed() => break None,
                _ = ready.notified() => {
                    info!("worker 已就绪");
                    backoff = 1.0;
                }
            }
        };

        let Some(status) = outcome else {
            info!("收到停止信号，正在结束 worker");
            let _ = child.start_kill();
            let _ = child.wait().await;
            return ExitCode::SUCCESS;
        };

        let status = match status {
            Ok(status) => status,
            Err(e) => {
                error!(error = %e, "等待 worker 失败");
                return ExitCode::FAILURE;
            }
        };
        if status.code() == Some(i32::from(FATAL_EXIT_CODE)) {
            error!("此配置下 worker 无法启动，不再重启");
            return ExitCode::FAILURE;
        }
        let jitter = 1.0 + (rand::random::<f64>() - 0.5) * 2.0 * BACKOFF_JITTER;
        backoff = (backoff * BACKOFF_FACTOR).min(BACKOFF_MAX) * jitter;
        warn!(
            status = %status,
            seconds = backoff.round(),
            "worker 已退出，正在重启"
        );
        tokio::time::sleep(Duration::from_secs_f64(backoff.max(1.0))).await;
    }
}
