//! One instance: register, verify once, serve.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
#[cfg(unix)]
use tracing::warn;
use tracing::{debug, error, info};

use crate::cluster::Cluster;
use crate::error::{Error, Result};
use crate::routes;
use crate::server::{self, HttpServer};
use crate::types::{CertPair, FileList};

/// Marker the supervisor watches for on the worker's stdout.
pub const READY_MARKER: &str = "OPENBMCLAPI_READY";

/// Interval between file-list refreshes.
const FILE_CHECK_INTERVAL: Duration = Duration::from_secs(600);

/// What an instance does about the shared file verification.
pub(super) enum Role {
    /// Runs the verification and tells the others how it went.
    Leader(watch::Sender<Option<bool>>),
    /// Waits for the leader, then goes straight to activation.
    Follower(watch::Receiver<Option<bool>>),
}

impl Role {
    /// The instance that verifies the files.
    pub(super) fn leader(gate: watch::Sender<Option<bool>>) -> Self {
        Role::Leader(gate)
    }

    /// An instance that waits for the leader.
    pub(super) fn follower(gate: watch::Receiver<Option<bool>>) -> Self {
        Role::Follower(gate)
    }
}

/// Run one instance to completion.
pub(super) async fn run(cluster: Arc<Cluster>, role: Role) -> Result<()> {
    let id = cluster.config.cluster_id.clone();
    info!(cluster_id = %id, port = cluster.config.port, "starting instance");

    cluster.spawn_event_listener();
    // Authenticate before anything else, like the Node agent.
    cluster.client.token_manager().get_token().await?;
    cluster.init().await?;
    cluster.connect().await;

    // The agent owns the public port and terminates TLS itself. Whatever sits
    // in front of it — a reverse proxy, a CDN, nothing — is the operator's call.
    let tls = match setup_certificate(&cluster).await? {
        Some(pair) => Some(server::tls_config(&pair.cert, &pair.key)?),
        None => None,
    };

    let router = routes::router(Arc::clone(&cluster));
    let bind = format!("0.0.0.0:{}", cluster.config.port);
    let http = Arc::new(HttpServer::bind(&bind, router, tls).await?);
    let serving = {
        let http = Arc::clone(&http);
        tokio::spawn(async move { http.serve().await })
    };

    cluster.port_check().await?;
    let files = prepare(&cluster, role).await?;

    info!(cluster_id = %id, "requesting activation");
    cluster.enable().await?;
    info!(cluster_id = %id, files = files.files.len(), "done, serving files");
    println!("{READY_MARKER}");

    let last = Arc::new(tokio::sync::Mutex::new(files));
    let checker = {
        let cluster = Arc::clone(&cluster);
        let last = Arc::clone(&last);
        tokio::spawn(async move { check_files(cluster, last).await })
    };

    wait_for_shutdown().await;
    info!(cluster_id = %id, "shutting down, unregistering cluster");
    checker.abort();
    if let Err(e) = cluster.disable().await {
        error!(error = %e, "failed to unregister");
    }
    http.close();
    let _ = serving.await;
    Ok(())
}

/// Establish the TLS material, or report that the agent serves plain HTTP.
async fn setup_certificate(cluster: &Arc<Cluster>) -> Result<Option<CertPair>> {
    if !cluster.config.byoc {
        info!("requesting certificate from the master");
        return cluster.request_cert().await.map(Some);
    }
    if cluster.config.ssl_cert.is_none() || cluster.config.ssl_key.is_none() {
        info!("BYOC without a certificate, falling back to HTTP");
        return Ok(None);
    }
    info!("using the supplied certificate");
    cluster.use_self_cert().await.map(Some)
}

/// Get the instance ready to serve.
async fn prepare(cluster: &Arc<Cluster>, role: Role) -> Result<FileList> {
    match role {
        Role::Leader(gate) => {
            let outcome = verify(cluster).await;
            // Release the followers either way: a verification that failed must
            // not leave them waiting forever.
            let _ = gate.send(Some(outcome.is_ok()));
            outcome
        }
        Role::Follower(mut gate) => {
            wait_for_verification(&mut gate).await?;
            let files = cluster.get_file_list(None).await?;
            info!(files = files.files.len(), "file list fetched");
            Ok(files)
        }
    }
}

/// Compare the storage against the master's list and collect the garbage.
async fn verify(cluster: &Arc<Cluster>) -> Result<FileList> {
    if !cluster.storage.check().await? {
        return Err(Error::storage("storage check failed"));
    }
    // Seed the probes now that the backend is known writable. The local cache
    // is skipped on purpose: it is the disk the agent already runs on, so the
    // route generates the payload instead of storing a second copy of it.
    if !cluster.config.uses_local_storage() && !cluster.config.measure_sizes.is_empty() {
        let written =
            crate::storage::measure::ensure(&*cluster.storage, &cluster.config.measure_sizes)
                .await?;
        info!(written, "seeded measure objects");
    }
    let configuration = cluster.get_configuration().await?;
    let files = cluster.get_file_list(None).await?;
    info!(files = files.files.len(), "file list fetched");
    cluster.sync_files(&files, &configuration.sync).await?;
    info!("collecting garbage");
    cluster.gc_background(files.clone());
    Ok(files)
}

/// Wait for the leader to finish verifying the files.
async fn wait_for_verification(gate: &mut watch::Receiver<Option<bool>>) -> Result<()> {
    if gate.borrow().is_none() {
        info!("waiting for the first instance to verify the files");
    }
    loop {
        let current = *gate.borrow();
        if let Some(verified) = current {
            return if verified {
                Ok(())
            } else {
                Err(Error::Other(
                    "the first instance could not verify the files".into(),
                ))
            };
        }
        if gate.changed().await.is_err() {
            return Err(Error::Other("the verification gate closed".into()));
        }
    }
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
