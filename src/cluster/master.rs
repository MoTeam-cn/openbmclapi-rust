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
}

/// Copy a certificate/key file, or write its inline contents.
async fn write_cert_material(source: &str, target: &std::path::Path) -> Result<()> {
    if std::path::Path::new(source).exists() {
        tokio::fs::copy(source, target).await?;
    } else {
        tokio::fs::write(target, source).await?;
    }
    Ok(())
}
