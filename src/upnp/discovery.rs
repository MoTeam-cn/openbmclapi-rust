//! SSDP discovery and UPnP device description parsing.

use std::net::Ipv4Addr;
use std::time::Duration;

use quick_xml::events::Event;
use quick_xml::Reader;
use tokio::net::UdpSocket;
use tracing::{debug, warn};

use crate::error::{Error, Result};

use super::xml::{header_value, local, local_ipv4_towards};

const SSDP_ADDR: &str = "239.255.255.250:1900";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);

/// A discovered Internet Gateway Device.
pub(super) struct Igd {
    /// Absolute control URL of the WAN connection service.
    pub(super) control_url: String,
    /// Service type string used for the SOAPAction header.
    pub(super) service_type: String,
    /// Local address used as `NewInternalClient`.
    pub(super) local_ip: Ipv4Addr,
    pub(super) http: reqwest::Client,
}

/// Run SSDP discovery and return the first usable WAN connection device.
pub(super) async fn discover() -> Result<Igd> {
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

/// Fetch and parse a device description, preferring WANIPConnection services.
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
