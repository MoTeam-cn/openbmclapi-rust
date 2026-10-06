use super::*;

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
