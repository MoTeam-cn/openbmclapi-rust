use super::*;

#[test]
fn extracts_blocks() {
    let pem = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
    let blocks = pem_blocks(pem, "CERTIFICATE");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0], vec![0, 0, 0]);
}
