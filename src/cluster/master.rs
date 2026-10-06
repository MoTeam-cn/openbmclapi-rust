//! Master-facing RPCs: file list, configuration, registration and certificates.

use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::Value;
use tracing::{info, trace};

use crate::error::{Error, Result};
use crate::filelist;
use crate::keepalive::split_ack;
use crate::types::{AgentConfiguration, CertPair, EnableRequest, FileList};

use super::cluster::Cluster;

/// Timeout for the `enable` acknowledgement.
const ENABLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Timeout for the `disable` acknowledgement.
const DISABLE_TIMEOUT: Duration = Duration::from_secs(60);

impl Cluster {
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
        trace!("上线");
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
        info!("开始干活");
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
    pub async fn request_cert(&self) -> Result<CertPair> {
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
        Ok(pair)
    }

    /// Read the BYOC certificate/key named by the configuration.
    pub async fn use_self_cert(&self) -> Result<CertPair> {
        let (Some(cert), Some(key)) = (self.config.ssl_cert.clone(), self.config.ssl_key.clone())
        else {
            return Err(Error::Config("缺少 SSL 证书或私钥".into()));
        };
        Ok(CertPair {
            cert: read_cert_material(&cert, "ssl_cert").await?,
            key: read_cert_material(&key, "ssl_key").await?,
        })
    }
}

/// Read certificate material: either a path to a file, or the PEM itself.
///
/// The pair is handed straight to the TLS setup, so nothing is staged on disk —
/// a private key in the temporary directory is a liability, not a convenience.
/// A path that exists but cannot be read is a configuration mistake and is
/// reported as one, so the supervisor does not loop on it.
async fn read_cert_material(source: &str, label: &str) -> Result<String> {
    if !std::path::Path::new(source).exists() {
        return Ok(source.to_string());
    }
    tokio::fs::read_to_string(source)
        .await
        .map_err(|e| Error::Config(format!("无法读取 {label} {source:?}：{e}")))
}
