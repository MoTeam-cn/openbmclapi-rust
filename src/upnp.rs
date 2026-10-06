//! Minimal UPnP IGD client for port mapping.
//!
//! Replaces `@xmcl/nat-api`: SSDP discovery, WANIPConnection/WANPPPConnection
//! selection, `AddPortMapping` and `GetExternalIPAddress`, with the mapping
//! renewed every 30 minutes.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use quick_xml::events::Event;
use quick_xml::Reader;
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};

use crate::error::{Error, Result};

const SSDP_ADDR: &str = "239.255.255.250:1900";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
const RENEW_INTERVAL: Duration = Duration::from_secs(30 * 60);
const LEASE_SECONDS: u32 = 3600;

/// A discovered Internet Gateway Device.
struct Igd {
    /// Absolute control URL of the WAN connection service.
    control_url: String,
    /// Service type string used for the SOAPAction header.
    service_type: String,
    /// Local address used as `NewInternalClient`.
    local_ip: Ipv4Addr,
    http: reqwest::Client,
}

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
                error!(error = %e, "upnp renewal failed");
            }
        }
    });
    Ok(external)
}

async fn discover() -> Result<Igd> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).await?;
    socket.set_broadcast(true)?;

    let search = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 3\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n";
    socket.send_to(search.as_bytes(), SSDP_ADDR).await?;

    let mut locations: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + DISCOVERY_TIMEOUT;
    let mut buffer = vec![0u8; 4096];
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.recv_from(&mut buffer)).await {
            Ok(Ok((len, from))) => {
                let text = String::from_utf8_lossy(&buffer[..len]);
                if let Some(location) = header_value(&text, "location") {
                    debug!(%from, location, "ssdp response");
                    if !locations.contains(&location) {
                        locations.push(location);
                    }
                }
            }
            Ok(Err(e)) => return Err(Error::Io(e)),
            Err(_) => break,
        }
    }

    if locations.is_empty() {
        return Err(Error::Other(
            "no UPnP gateway answered SSDP discovery".into(),
        ));
    }

    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()?;
    let local_ip = local_ipv4_towards(&locations[0]).unwrap_or(Ipv4Addr::LOCALHOST);

    for location in locations {
        match describe(&http, &location, local_ip).await {
            Ok(igd) => return Ok(igd),
            Err(e) => warn!(error = %e, location, "unusable UPnP device"),
        }
    }
    Err(Error::Other(
        "no usable UPnP WAN connection service found".into(),
    ))
}

async fn describe(http: &reqwest::Client, location: &str, local_ip: Ipv4Addr) -> Result<Igd> {
    let body = http
        .get(location)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let base = url::Url::parse(location)?;

    let mut reader = Reader::from_str(&body);
    let mut current_service: Option<String> = None;
    let mut service_type: Option<String> = None;
    let mut control_url: Option<String> = None;
    let mut text_target: Option<&'static str> = None;
    let mut fallback: Option<(String, String)> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local(event.name().as_ref());
                match name.as_str() {
                    "service" => {
                        current_service = None;
                        service_type = None;
                        control_url = None;
                    }
                    "servicetype" => text_target = Some("serviceType"),
                    "controlurl" => text_target = Some("controlURL"),
                    _ => {}
                }
            }
            Ok(Event::Text(event)) => {
                if let Some(target) = text_target {
                    let value = String::from_utf8_lossy(event.as_ref()).trim().to_string();
                    match target {
                        "serviceType" => service_type = Some(value),
                        "controlURL" => control_url = Some(value),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(event)) => {
                let name = local(event.name().as_ref());
                if name == "servicetype" || name == "controlurl" {
                    text_target = None;
                } else if name == "service" {
                    if let (Some(st), Some(cu)) = (service_type.take(), control_url.take()) {
                        if st.contains("WANIPConnection") || st.contains("WANPPPConnection") {
                            let absolute = base.join(&cu)?.to_string();
                            if st.contains("WANIPConnection") {
                                return Ok(Igd {
                                    control_url: absolute,
                                    service_type: st,
                                    local_ip,
                                    http: http.clone(),
                                });
                            }
                            fallback = Some((st, absolute));
                        }
                    }
                    current_service = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Error::Other(format!("invalid UPnP description: {e}"))),
            _ => {}
        }
    }
    let _ = current_service;

    match fallback {
        Some((service_type, control_url)) => Ok(Igd {
            control_url,
            service_type,
            local_ip,
            http: http.clone(),
        }),
        None => Err(Error::Other(
            "device exposes no WAN connection service".into(),
        )),
    }
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
    info!(port, public_port, "upnp port mapped");
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

/// Pull a header value out of an SSDP response.
fn header_value(response: &str, header: &str) -> Option<String> {
    for line in response.lines() {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case(header) {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

fn local(tag: &[u8]) -> String {
    let text = String::from_utf8_lossy(tag);
    match text.rsplit_once(':') {
        Some((_, local)) => local.to_ascii_lowercase(),
        None => text.to_ascii_lowercase(),
    }
}

/// Extract the text of the first occurrence of `<tag>` (namespace agnostic).
fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let target = tag.to_ascii_lowercase();
    let mut capture = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if local(event.name().as_ref()) == target => {
                capture = true;
            }
            Ok(Event::Text(event)) if capture => {
                return Some(String::from_utf8_lossy(event.as_ref()).trim().to_string());
            }
            Ok(Event::Eof) => return None,
            Err(_) => return None,
            _ => {}
        }
    }
}

/// Determine the local IPv4 address used to reach `remote`.
fn local_ipv4_towards(remote: &str) -> Option<Ipv4Addr> {
    let host = url::Url::parse(remote).ok()?.host_str()?.to_string();
    let addrs = std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), 80)).ok()?;
    let target: SocketAddr = addrs.into_iter().find(|addr| addr.is_ipv4())?;
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(target).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ssdp_headers() {
        let response = "HTTP/1.1 200 OK\r\nLOCATION: http://192.168.1.1:1900/rootDesc.xml\r\nST: upnp:rootdevice\r\n";
        assert_eq!(
            header_value(response, "location").as_deref(),
            Some("http://192.168.1.1:1900/rootDesc.xml")
        );
    }

    #[test]
    fn extracts_external_ip() {
        let xml = "<s:Envelope><s:Body><u:GetExternalIPAddressResponse><NewExternalIPAddress>203.0.113.7</NewExternalIPAddress></u:GetExternalIPAddressResponse></s:Body></s:Envelope>";
        assert_eq!(
            extract_tag(xml, "NewExternalIPAddress").as_deref(),
            Some("203.0.113.7")
        );
    }
}
