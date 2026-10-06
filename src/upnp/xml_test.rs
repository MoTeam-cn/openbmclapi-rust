use super::{extract_tag, header_value};

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
