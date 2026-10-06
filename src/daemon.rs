//! Daemon supervisor: keeps a worker process alive and dispatches the entry point.

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

const BACKOFF_FACTOR: f64 = 2.0;
const BACKOFF_MAX: f64 = 60.0;
const BACKOFF_JITTER: f64 = 0.2;

/// Load configuration and either run a worker or supervise one.
pub fn entry() -> ExitCode {
    let args = Cli::parse_args();
    if let Some(CliCommand::Init { force }) = &args.command {
        return match cli::write_config(&args.init_target(), *force) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("cannot write the configuration: {e}");
                ExitCode::FAILURE
            }
        };
    }

    let _ = dotenvy::dotenv();
    let config_path = match args.config_path() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("configuration error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let format = logger::LogFormat::parse(std::env::var("LOG_FORMAT").ok().as_deref());
    logger::init(&config.log_level, config.plain_log, format);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    if config.no_daemon || std::env::var(WORKER_ENV).is_ok() {
        return match runtime.block_on(bootstrap::run(config)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                error!(error = %e, "bootstrap failed");
                ExitCode::FAILURE
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
            error!(error = %e, "cannot resolve the current executable");
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
                error!(error = %e, "cannot spawn the worker process");
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
                    info!("worker reported ready");
                    backoff = 1.0;
                }
            }
        };

        let Some(status) = outcome else {
            info!("received a stop signal, terminating the worker");
            let _ = child.start_kill();
            let _ = child.wait().await;
            return ExitCode::SUCCESS;
        };

        let status = match status {
            Ok(status) => status,
            Err(e) => {
                error!(error = %e, "worker wait failed");
                return ExitCode::FAILURE;
            }
        };
        let jitter = 1.0 + (rand::random::<f64>() - 0.5) * 2.0 * BACKOFF_JITTER;
        backoff = (backoff * BACKOFF_FACTOR).min(BACKOFF_MAX) * jitter;
        warn!(
            status = %status,
            seconds = backoff.round(),
            "worker exited, restarting"
        );
        tokio::time::sleep(Duration::from_secs_f64(backoff.max(1.0))).await;
    }
}
