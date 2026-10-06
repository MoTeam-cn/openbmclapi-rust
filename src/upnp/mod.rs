//! Minimal UPnP IGD client for port mapping.
//!
//! Replaces `@xmcl/nat-api`: SSDP discovery, WANIPConnection/WANPPPConnection
//! selection, `AddPortMapping` and `GetExternalIPAddress`, with the mapping
//! renewed every 30 minutes.

mod discovery;
mod soap;
mod xml;

pub use soap::setup_upnp;
