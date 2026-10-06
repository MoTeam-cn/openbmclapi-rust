//! The `Cluster` type and its lifecycle.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};
use tracing::{debug, error, info, warn};

use crate::client::BmclapiClient;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::keepalive::Keepalive;
use crate::socketio::{SocketEvent, SocketIo};
use crate::storage::{self, Storage};
use crate::token::TokenManager;
use crate::types::Counters;

use super::checksum::is_unicast;

/// The running agent.
pub struct Cluster {
    /// Effective configuration the agent was started with.
    pub config: Config,
    /// Backend that stores and serves the downloaded objects.
    pub storage: Arc<dyn Storage>,
    /// HTTP client talking to the master.
    pub client: BmclapiClient,
    /// Socket.IO connection to the master.
    pub socket: SocketIo,
    /// Keep-alive reporter started after a successful `enable`.
    pub keepalive: Arc<Keepalive>,
    /// Agent version reported to the master.
    pub version: String,

    /// Bytes/hits accumulated since the last successful report.
    pub(super) counters: Mutex<Counters>,
    /// Whether the master currently accepts this agent.
    pub(super) is_enabled: AtomicBool,
    /// Whether the agent should re-register after a reconnect.
    pub(super) want_enable: AtomicBool,
    /// Advertised host, overridden by UPnP when that is enabled.
    pub(super) host: Mutex<Option<String>>,
    /// Per-hash locks so concurrent requests download an object once.
    pub(super) download_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Set once shutdown has started so it is only performed once.
    pub(super) shutting_down: AtomicBool,
}

impl Cluster {
    /// Build the cluster and all of its collaborators.
    pub fn new(config: Config) -> Result<Arc<Self>> {
        let token = TokenManager::new(
            config.cluster_id.clone(),
            config.cluster_secret.clone(),
            crate::VERSION,
            config.bmclapi_base.clone(),
        )?;
        let client = BmclapiClient::new(&config, token.clone())?;
        let socket = SocketIo::new(&config.bmclapi_base, token);
        let storage = storage::create(&config)?;
        let host = config.cluster_ip.clone();
        let version = crate::VERSION.to_string();

        let cluster = Arc::new_cyclic(|weak| Cluster {
            config,
            storage,
            client,
            socket,
            keepalive: Arc::new(Keepalive::new(weak.clone())),
            version,
            counters: Mutex::new(Counters::default()),
            is_enabled: AtomicBool::new(false),
            want_enable: AtomicBool::new(false),
            host: Mutex::new(host),
            download_locks: Mutex::new(HashMap::new()),
            shutting_down: AtomicBool::new(false),
        });
        Ok(cluster)
    }

    /// Whether the agent is currently registered with the master.
    pub fn is_enabled(&self) -> bool {
        self.is_enabled.load(Ordering::Relaxed)
    }

    /// Storage warm-up plus optional UPnP port mapping.
    pub async fn init(self: &Arc<Self>) -> Result<()> {
        self.storage.init().await?;
        if self.config.enable_upnp {
            let ip =
                crate::upnp::setup_upnp(self.config.port, self.config.cluster_public_port).await?;
            let addr: IpAddr = ip
                .parse()
                .map_err(|_| Error::Other(format!("UPnP returned an invalid IP: {ip}")))?;
            if !addr.is_ipv4() {
                return Err(Error::Other("IPv6 is not supported".into()));
            }
            if !is_unicast(&addr) {
                return Err(Error::Other(format!(
                    "UPnP returned a non-public address: {ip}"
                )));
            }
            info!(ip = %ip, "upnp mapping established");
            *self.host.lock().await = Some(ip);
        }
        Ok(())
    }

    /// Listen for socket events and react the way the Node agent does.
    pub fn spawn_event_listener(self: &Arc<Self>) {
        let mut events = self.socket.subscribe();
        let cluster = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(SocketEvent::Connected) => debug!("socket connected"),
                    Ok(SocketEvent::Message(payload)) => {
                        info!(payload = %payload, "master message")
                    }
                    Ok(SocketEvent::WardenError(payload)) => {
                        warn!(payload = %payload, "master reported a warden error")
                    }
                    Ok(SocketEvent::Exception(payload)) => {
                        error!(payload = %payload, "server exception")
                    }
                    Ok(SocketEvent::ConnectError(payload)) => {
                        error!(payload = %payload, "the master refused the connection");
                        cluster.is_enabled.store(false, Ordering::Relaxed);
                        cluster.keepalive.stop();
                    }
                    Ok(SocketEvent::Disconnected(reason)) => {
                        warn!(reason, "disconnected from master");
                        cluster.is_enabled.store(false, Ordering::Relaxed);
                        cluster.keepalive.stop();
                    }
                    Ok(SocketEvent::Reconnect(attempt)) => {
                        info!(attempt, "reconnected to master");
                        if cluster.want_enable.load(Ordering::Relaxed) {
                            info!("re-enabling after reconnect");
                            if let Err(e) = cluster.enable().await {
                                error!(error = %e, "reconnect: cannot connect to server");
                                cluster.exit(1);
                            }
                        }
                    }
                    Ok(SocketEvent::ReconnectError(e)) => error!(error = %e, "reconnect_error"),
                    Ok(SocketEvent::ReconnectFailed) => {
                        error!("reconnect failed");
                        cluster.exit(1);
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "socket event listener lagged")
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Connect the socket (idempotent).
    pub async fn connect(&self) {
        self.socket.connect().await;
    }

    /// Terminate the process.
    pub fn exit(&self, code: i32) -> ! {
        self.shutting_down.store(true, Ordering::Relaxed);
        std::process::exit(code);
    }

    /// Whether a shutdown is already in progress.
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
#[path = "cluster_test.rs"]
mod tests;
