//! Worker bootstrap: register, sync, serve.

use std::sync::Arc;
use std::time::Duration;

#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use crate::cluster::Cluster;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::routes;
use crate::server::{self, HttpServer};
use crate::types::FileList;

/// Marker the supervisor watches for on the worker's stdout.
pub const READY_MARKER: &str = "OPENBMCLAPI_READY";

/// Interval between file-list refreshes.
const FILE_CHECK_INTERVAL: Duration = Duration::from_secs(600);

/// Run one agent process to completion.
pub async fn run(config: Config) -> Result<()> {
    info!("booting openbmclapi {}", crate::VERSION);
    server::install_crypto_provider();

    let cluster = Cluster::new(config)?;
    cluster.spawn_event_listener();

    // Authenticate before anything else, like the Node agent.
    cluster.client.token_manager().get_token().await?;

    cluster.init().await?;
    cluster.connect().await;

    let mut use_https = true;
    if cluster.config.byoc {
        if cluster.config.ssl_cert.is_none() || cluster.config.ssl_key.is_none() {
            info!("BYOC without a certificate, falling back to HTTP");
            use_https = false;
        } else {
            info!("using the supplied certificate");
            cluster.use_self_cert().await?;
        }
    } else {
        info!("requesting certificate from the master");
        cluster.request_cert().await?;
    }

    // nginx terminates TLS and owns the public port; the agent then speaks
    // plain HTTP on a loopback port that only nginx can reach.
    let behind_nginx = cluster.config.enable_nginx;
    let tls = if use_https && !behind_nginx {
        let dir = cluster.config.tmp_dir();
        let cert = tokio::fs::read_to_string(dir.join("cert.pem")).await?;
        let key = tokio::fs::read_to_string(dir.join("key.pem")).await?;
        Some(server::tls_config(&cert, &key)?)
    } else {
        None
    };

    let router = routes::router(Arc::clone(&cluster));
    let bind = if behind_nginx {
        "127.0.0.1:0".to_string()
    } else {
        format!("0.0.0.0:{}", cluster.config.port)
    };
    let http = Arc::new(HttpServer::bind(&bind, router, tls).await?);
    let serving = {
        let http = Arc::clone(&http);
        tokio::spawn(async move { http.serve().await })
    };

    // The public port belongs to nginx, so it can only start once the agent
    // knows which loopback port it landed on.
    let mut nginx = if behind_nginx {
        let app_port = http.local_addr()?.port();
        info!(
            public_port = cluster.config.port,
            app_port, "starting the nginx front-end"
        );
        Some(crate::nginx::setup(&cluster, cluster.config.port, use_https, app_port).await?)
    } else {
        None
    };

    cluster.port_check().await?;

    if !cluster.storage.check().await? {
        return Err(Error::storage("storage check failed"));
    }

    let configuration = cluster.get_configuration().await?;
    let files = cluster.get_file_list(None).await?;
    info!(files = files.files.len(), "file list fetched");
    cluster.sync_files(&files, &configuration.sync).await?;
    info!("collecting garbage");
    cluster.gc_background(files.clone());

    info!("requesting activation");
    cluster.enable().await?;
    info!(files = files.files.len(), "done, serving files");
    println!("{READY_MARKER}");

    let last = Arc::new(tokio::sync::Mutex::new(files));
    let checker = {
        let cluster = Arc::clone(&cluster);
        let last = Arc::clone(&last);
        tokio::spawn(async move { check_files(cluster, last).await })
    };

    wait_for_shutdown().await;
    info!("shutting down, unregistering cluster");
    checker.abort();
    if let Some(nginx) = nginx.as_mut() {
        nginx.shutdown().await;
    }
    if let Err(e) = cluster.disable().await {
        error!(error = %e, "failed to unregister");
    }
    http.close();
    let _ = serving.await;
    Ok(())
}

/// Refresh the file list every ten minutes.
async fn check_files(cluster: Arc<Cluster>, last: Arc<tokio::sync::Mutex<FileList>>) {
    loop {
        tokio::time::sleep(FILE_CHECK_INTERVAL).await;
        debug!("refresh files");
        let last_modified = last.lock().await.files.iter().map(|file| file.mtime).max();
        match cluster.get_file_list(last_modified).await {
            Ok(list) if list.files.is_empty() => debug!("no new files"),
            Ok(list) => {
                let configuration = match cluster.get_configuration().await {
                    Ok(configuration) => configuration,
                    Err(e) => {
                        error!(error = %e, "failed to fetch configuration");
                        continue;
                    }
                };
                if let Err(e) = cluster.sync_files(&list, &configuration.sync).await {
                    error!(error = %e, "sync failed");
                    continue;
                }
                *last.lock().await = list;
            }
            Err(e) => error!(error = %e, "failed to refresh the file list"),
        }
    }
}

/// Resolve on Ctrl+C or (on unix) SIGTERM.
pub async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(term) => term,
            Err(e) => {
                warn!(error = %e, "cannot listen for SIGTERM");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
