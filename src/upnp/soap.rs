//! SOAP port-mapping calls and the periodic renewal loop.

use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info};

use crate::error::{Error, Result};

use super::discovery::{discover, Igd};
use super::xml::extract_tag;

const RENEW_INTERVAL: Duration = Duration::from_secs(30 * 60);
const LEASE_SECONDS: u32 = 3600;

/// Map a port, start the renewal loop and return the external IP.
pub async fn setup_upnp(port: u16, public_port: u16) -> Result<String> {
    let igd = Arc::new(discover().await?);
    map_port(&igd, port, public_port).await?;
    let external = external_ip(&igd).await?;

    let renew = Arc::clone(&igd);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RENEW_INTERVAL).await;
            if let Err(e) = map_port(&renew, port, public_port).await {
                error!(error = %e, "UPnP 续期失败");
            }
        }
    });
    Ok(external)
}

async fn map_port(igd: &Igd, port: u16, public_port: u16) -> Result<()> {
    let body = format!(
        r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body>
<u:AddPortMapping xmlns:u="{service}">
<NewRemoteHost></NewRemoteHost>
<NewExternalPort>{public_port}</NewExternalPort>
<NewProtocol>TCP</NewProtocol>
<NewInternalPort>{port}</NewInternalPort>
<NewInternalClient>{client}</NewInternalClient>
<NewEnabled>1</NewEnabled>
<NewPortMappingDescription>openbmclapi</NewPortMappingDescription>
<NewLeaseDuration>{lease}</NewLeaseDuration>
</u:AddPortMapping>
</s:Body>
</s:Envelope>"#,
        service = igd.service_type,
        client = igd.local_ip,
        lease = LEASE_SECONDS,
    );
    let response = igd
        .http
        .post(&igd.control_url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header(
            "SOAPAction",
            format!("\"{}#AddPortMapping\"", igd.service_type),
        )
        .body(body)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(Error::Other(format!(
            "AddPortMapping failed ({status}): {text}"
        )));
    }
    info!(port, public_port, "UPnP 端口映射完成");
    Ok(())
}

async fn external_ip(igd: &Igd) -> Result<String> {
    let body = format!(
        r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body>
<u:GetExternalIPAddress xmlns:u="{service}"></u:GetExternalIPAddress>
</s:Body>
</s:Envelope>"#,
        service = igd.service_type,
    );
    let response = igd
        .http
        .post(&igd.control_url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header(
            "SOAPAction",
            format!("\"{}#GetExternalIPAddress\"", igd.service_type),
        )
        .body(body)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    extract_tag(&response, "NewExternalIPAddress")
        .ok_or_else(|| Error::Other("gateway did not return an external IP".into()))
}
