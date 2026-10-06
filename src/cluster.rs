//! Cluster orchestration: registration, file sync, downloads and lifecycle.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use tokio::sync::{broadcast, Mutex};
use tracing::{debug, error, info, trace, warn};

use crate::client::BmclapiClient;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::filelist;
use crate::keepalive::{split_ack, Keepalive};
use crate::socketio::{SocketEvent, SocketIo};
use crate::storage::{self, ServeStat, Storage};
use crate::token::TokenManager;
use crate::types::{
    AgentConfiguration, CertPair, Counters, EnableRequest, FileInfo, FileList, SyncConfig,
};
use crate::util::{hash_to_filename, now_ms};

/// Retries per file when syncing, matching `p-retry` in the Node agent.
const SYNC_RETRIES: u32 = 10;
/// Timeout for the `enable` acknowledgement.
const ENABLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Timeout for the `disable` acknowledgement.
const DISABLE_TIMEOUT: Duration = Duration::from_secs(60);

/// The running agent.
pub struct Cluster {
    pub config: Config,
    pub storage: Arc<dyn Storage>,
    pub client: BmclapiClient,
    pub socket: SocketIo,
    pub keepalive: Arc<Keepalive>,
    pub version: String,

    counters: Mutex<Counters>,
    is_enabled: AtomicBool,
    want_enable: AtomicBool,
    host: Mutex<Option<String>>,
    download_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    shutting_down: AtomicBool,
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

    /// Fetch the master file list, optionally only entries newer than a time.
    pub async fn get_file_list(&self, last_modified: Option<i64>) -> Result<FileList> {
        let query: Vec<(&str, String)> = last_modified
            .map(|value| vec![("lastModified", value.to_string())])
            .unwrap_or_default();
        let fetched = self
            .client
            .get_bytes("openbmclapi/files", &query, true)
            .await?;
        if fetched.status.as_u16() == 204 {
            return Ok(FileList::default());
        }
        if !fetched.is_success() {
            return Err(Error::Status {
                status: fetched.status.as_u16(),
                url: "openbmclapi/files".into(),
            });
        }
        let decompressed = zstd::decode_all(fetched.body.as_ref())
            .map_err(|e| Error::Other(format!("zstd decode failed: {e}")))?;
        let files = filelist::decode(&decompressed)?;
        Ok(FileList { files })
    }

    /// Fetch the master-provided sync configuration.
    pub async fn get_configuration(&self) -> Result<AgentConfiguration> {
        self.client
            .get_json("openbmclapi/configuration", &[], true)
            .await
    }

    /// Download every file that is missing or size-mismatched.
    pub async fn sync_files(&self, file_list: &FileList, sync: &SyncConfig) -> Result<()> {
        if !self.storage.check().await? {
            return Err(Error::storage("storage is not writable"));
        }
        info!("checking for missing files");
        let missing = self.storage.get_missing_files(&file_list.files).await?;
        if missing.is_empty() {
            return Ok(());
        }
        info!(count = missing.len(), "mismatch found, starting sync");
        info!(concurrency = sync.concurrency, "sync strategy");

        let concurrency = sync.concurrency.max(1);
        let total = missing.len();
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let has_error = Arc::new(AtomicBool::new(false));

        let results = futures::stream::iter(missing.into_iter())
            .map(|file| {
                let done = Arc::clone(&done);
                let has_error = Arc::clone(&has_error);
                async move {
                    let outcome = self.sync_one(&file).await;
                    let processed = done.fetch_add(1, Ordering::Relaxed) + 1;
                    match &outcome {
                        Ok(()) => trace!(path = %file.path, "synced"),
                        Err(e) => {
                            has_error.store(true, Ordering::Relaxed);
                            error!(error = %e, path = %file.path, "failed to download file");
                        }
                    }
                    if processed % 100 == 0 || processed == total {
                        info!(processed, total, "sync progress");
                    }
                    outcome
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;
        drop(results);

        if has_error.load(Ordering::Relaxed) {
            Err(Error::Other("sync failed".into()))
        } else {
            info!("sync complete");
            Ok(())
        }
    }

    async fn sync_one(&self, file: &FileInfo) -> Result<()> {
        let mut attempt = 0u32;
        loop {
            match self.download_and_store(file).await {
                Ok(()) => return Ok(()),
                Err(failure) => {
                    attempt += 1;
                    // The master wants to know when a download ends up on a
                    // different URL than the one it handed out.
                    if let Some(final_url) = failure.redirect.clone() {
                        self.report_redirect(file, &failure.error, final_url).await;
                    }
                    if attempt > SYNC_RETRIES {
                        return Err(failure.error);
                    }
                    debug!(
                        error = %failure.error,
                        path = %file.path,
                        attempt,
                        "download failed, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await;
                }
            }
        }
    }

    async fn download_and_store(
        &self,
        file: &FileInfo,
    ) -> std::result::Result<(), DownloadFailure> {
        let path = file.path.trim_start_matches('/');
        let requested = format!("{}/{}", self.client.base(), path);
        let response = self
            .client
            .get_stream(path, &[])
            .await
            .map_err(DownloadFailure::new)?;

        // `reqwest` follows redirects; the final URL reveals whether the object
        // actually came from the master or from a CDN.
        let final_url = response.url().to_string();
        let redirect = (final_url != requested).then_some(final_url);

        let status = response.status();
        if !status.is_success() {
            return Err(DownloadFailure {
                error: Error::Status {
                    status: status.as_u16(),
                    url: file.path.clone(),
                },
                redirect,
            });
        }

        let mut body = Vec::with_capacity(file.size.max(0) as usize);
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| DownloadFailure::new(e.into()))?
        {
            body.extend_from_slice(&chunk);
        }
        if !validate_file(&body, &file.hash) {
            return Err(DownloadFailure {
                error: Error::Other(format!("checksum mismatch for {}", file.path)),
                redirect,
            });
        }
        self.storage
            .write_file(&hash_to_filename(&file.hash), &body, file)
            .await
            .map_err(|error| DownloadFailure { error, redirect })
    }

    /// Tell the master that `file` was served from `final_url` instead of the
    /// URL it advertised.
    async fn report_redirect(&self, file: &FileInfo, error: &Error, final_url: String) {
        let requested = format!(
            "{}/{}",
            self.client.base(),
            file.path.trim_start_matches('/')
        );
        let payload = json!({
            "urls": [requested, final_url],
            "error": serde_json::to_string(&json!({ "message": error.to_string() })).unwrap_or_default(),
        });
        if let Err(e) = self.client.report(payload).await {
            error!(error = %e, "failed to report redirect");
        }
    }

    /// Ensure an object is present locally, de-duplicating concurrent requests.
    pub async fn ensure_downloaded(&self, hash: &str) -> Result<()> {
        let hash_path = hash_to_filename(hash);
        if self.storage.exists(&hash_path).await? {
            return Ok(());
        }

        let (lock, fresh) = {
            let mut locks = self.download_locks.lock().await;
            match locks.get(hash) {
                Some(existing) => (Arc::clone(existing), false),
                None => {
                    let created = Arc::new(Mutex::new(()));
                    locks.insert(hash.to_string(), Arc::clone(&created));
                    (created, true)
                }
            }
        };

        let guard = lock.lock().await;
        if self.storage.exists(&hash_path).await? {
            drop(guard);
            if fresh {
                self.download_locks.lock().await.remove(hash);
            }
            return Ok(());
        }
        let result = self.download_file(hash).await;
        drop(guard);
        if fresh {
            self.download_locks.lock().await.remove(hash);
        }
        result
    }

    /// Fetch a single object from the master on demand.
    pub async fn download_file(&self, hash: &str) -> Result<()> {
        let response = self
            .client
            .get_stream(
                &format!("openbmclapi/download/{hash}"),
                &[("noopen", "1".to_string())],
            )
            .await?;
        let status = response.status();
        if status.as_u16() == 404 {
            return Err(Error::NotFound);
        }
        if !status.is_success() {
            return Err(Error::Status {
                status: status.as_u16(),
                url: format!("openbmclapi/download/{hash}"),
            });
        }
        let body = response.bytes().await?;
        let info = FileInfo {
            path: format!("/download/{hash}"),
            hash: hash.to_string(),
            size: body.len() as i64,
            mtime: now_ms(),
        };
        self.storage
            .write_file(&hash_to_filename(hash), &body, &info)
            .await
    }

    /// Record bytes served by a download.
    pub async fn record_served(&self, stat: ServeStat) {
        let mut counters = self.counters.lock().await;
        counters.hits += stat.hits;
        counters.bytes += stat.bytes;
    }

    /// Snapshot the counters without resetting them.
    pub async fn take_counters_snapshot(&self) -> Counters {
        *self.counters.lock().await
    }

    /// Subtract the reported counters after a successful keep-alive.
    pub async fn subtract_counters(&self, reported: Counters) {
        let mut counters = self.counters.lock().await;
        counters.hits = counters.hits.saturating_sub(reported.hits);
        counters.bytes = counters.bytes.saturating_sub(reported.bytes);
    }

    async fn enable_payload(&self) -> EnableRequest {
        EnableRequest {
            host: self.host.lock().await.clone(),
            port: self.config.cluster_public_port,
            version: self.version.clone(),
            byoc: self.config.byoc,
            no_fast_enable: self.config.no_fast_enable,
            flavor: self.config.flavor.clone(),
        }
    }

    /// Ask the master to verify the advertised address and port.
    pub async fn port_check(&self) -> Result<()> {
        let payload = serde_json::to_value(self.enable_payload().await)?;
        let ack = self
            .socket
            .emit_with_ack("port-check", payload, DISABLE_TIMEOUT)
            .await?;
        let (err, ack) = split_ack(&ack);
        if let Some(err) = err {
            return Err(Error::Service(
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("port check failed")
                    .to_string(),
            ));
        }
        if ack != Some(&Value::Bool(true)) {
            return Err(Error::Other("port check failed".into()));
        }
        Ok(())
    }

    /// Register with the master and start reporting.
    pub async fn enable(&self) -> Result<()> {
        if self.is_enabled() {
            return Ok(());
        }
        trace!("enable");
        let payload = serde_json::to_value(self.enable_payload().await)?;
        let ack = self
            .socket
            .emit_with_ack("enable", payload, ENABLE_TIMEOUT)
            .await?;
        let (err, ack) = split_ack(&ack);
        if let Some(err) = err {
            return Err(Error::Service(
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("enable failed")
                    .to_string(),
            ));
        }
        if ack != Some(&Value::Bool(true)) {
            return Err(Error::Other("failed to register cluster".into()));
        }
        info!("start doing my job");
        self.is_enabled.store(true, Ordering::Relaxed);
        self.want_enable.store(true, Ordering::Relaxed);
        self.keepalive.start();
        Ok(())
    }

    /// Unregister from the master and close the socket.
    pub async fn disable(&self) -> Result<()> {
        self.keepalive.stop();
        self.want_enable.store(false, Ordering::Relaxed);
        if !self.socket.is_connected() {
            self.is_enabled.store(false, Ordering::Relaxed);
            self.socket.disconnect().await;
            return Ok(());
        }
        let ack = self
            .socket
            .emit_with_ack("disable", Value::Null, DISABLE_TIMEOUT)
            .await?;
        self.is_enabled.store(false, Ordering::Relaxed);
        self.socket.disconnect().await;
        let (err, ack) = split_ack(&ack);
        if let Some(err) = err {
            return Err(Error::Service(
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("disable failed")
                    .to_string(),
            ));
        }
        if ack != Some(&Value::Bool(true)) {
            return Err(Error::Other("failed to disable cluster".into()));
        }
        Ok(())
    }

    /// Ask the master for a TLS certificate pair.
    pub async fn request_cert(&self) -> Result<()> {
        let ack = self
            .socket
            .emit_with_ack("request-cert", Value::Null, DISABLE_TIMEOUT)
            .await?;
        let (err, value) = split_ack(&ack);
        if let Some(err) = err {
            return Err(Error::Service(
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("request-cert failed")
                    .to_string(),
            ));
        }
        let pair: CertPair = serde_json::from_value(value.cloned().unwrap_or(Value::Null))?;
        let dir = self.config.tmp_dir();
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(dir.join("cert.pem"), pair.cert).await?;
        tokio::fs::write(dir.join("key.pem"), pair.key).await?;
        Ok(())
    }

    /// Materialise a BYOC certificate/key into the working directory.
    pub async fn use_self_cert(&self) -> Result<()> {
        let (Some(cert), Some(key)) = (self.config.ssl_cert.clone(), self.config.ssl_key.clone())
        else {
            return Err(Error::Config("missing SSL certificate or key".into()));
        };
        let dir = self.config.tmp_dir();
        tokio::fs::create_dir_all(&dir).await?;
        write_cert_material(&cert, &dir.join("cert.pem")).await?;
        write_cert_material(&key, &dir.join("key.pem")).await?;
        Ok(())
    }

    /// Reclaim disk space in the background.
    pub fn gc_background(self: &Arc<Self>, files: FileList) {
        let cluster = Arc::clone(self);
        tokio::spawn(async move {
            match cluster.storage.gc(&files.files).await {
                Ok(counter) if counter.count == 0 => info!("no expired files"),
                Ok(counter) => info!(
                    count = counter.count,
                    bytes = counter.size,
                    "garbage collection complete"
                ),
                Err(e) => error!(error = %e, "gc error"),
            }
        });
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

async fn write_cert_material(source: &str, target: &std::path::Path) -> Result<()> {
    if std::path::Path::new(source).exists() {
        tokio::fs::copy(source, target).await?;
    } else {
        tokio::fs::write(target, source).await?;
    }
    Ok(())
}

/// A failed file download, carrying the redirect target when there was one.
struct DownloadFailure {
    error: Error,
    redirect: Option<String>,
}

impl DownloadFailure {
    fn new(error: Error) -> Self {
        DownloadFailure {
            error,
            redirect: None,
        }
    }
}

/// Verify a downloaded buffer against the master checksum.
///
/// A 32 character hash is MD5, anything else is SHA-1 — matching `file.ts`.
pub fn validate_file(buffer: &[u8], checksum: &str) -> bool {
    if checksum.len() == 32 {
        format!("{:x}", md5::compute(buffer)) == checksum
    } else {
        let mut hasher = Sha1::new();
        hasher.update(buffer);
        hex::encode(hasher.finalize()) == checksum
    }
}

/// `ipaddr.range() === 'unicast'` equivalent for IPv4/IPv6.
pub fn is_unicast(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.octets()[0] >= 240)
        }
        IpAddr::V6(v6) => !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cluster is shared across tasks, so this must keep holding.
    #[test]
    fn cluster_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Cluster>();
    }

    #[test]
    fn validates_md5_and_sha1() {
        let data = b"hello world";
        assert!(validate_file(data, &format!("{:x}", md5::compute(data))));
        let mut hasher = Sha1::new();
        hasher.update(data);
        assert!(validate_file(data, &hex::encode(hasher.finalize())));
        assert!(!validate_file(data, "deadbeef"));
    }

    #[test]
    fn detects_private_addresses() {
        assert!(!is_unicast(&"192.168.1.1".parse().unwrap()));
        assert!(!is_unicast(&"10.0.0.1".parse().unwrap()));
        assert!(!is_unicast(&"127.0.0.1".parse().unwrap()));
        assert!(is_unicast(&"1.1.1.1".parse().unwrap()));
    }
}
