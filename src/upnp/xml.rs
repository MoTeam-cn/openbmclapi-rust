//! XML/URL helpers shared by SSDP discovery and SOAP request handling.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use quick_xml::events::Event;
use quick_xml::Reader;

/// Pull a header value out of an SSDP response.
pub(super) fn header_value(response: &str, header: &str) -> Option<String> {
    for line in response.lines() {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case(header) {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// Strip any namespace prefix and lowercase a qualified XML name.
pub(super) fn local(tag: &[u8]) -> String {
    let text = String::from_utf8_lossy(tag);
    match text.rsplit_once(':') {
        Some((_, local)) => local.to_ascii_lowercase(),
        None => text.to_ascii_lowercase(),
    }
}

/// Extract the text of the first occurrence of `<tag>` (namespace agnostic).
pub(super) fn extract_tag(xml: &str, tag: &str) -> Option<String> {
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
pub(super) fn local_ipv4_towards(remote: &str) -> Option<Ipv4Addr> {
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
#[path = "xml_test.rs"]
mod tests;
