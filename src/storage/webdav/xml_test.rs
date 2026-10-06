use super::*;

#[test]
fn parses_multistatus() {
    let body = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/cache/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/cache/ab/abcdef</D:href>
    <D:propstat><D:prop><D:resourcetype/><D:getcontentlength>123</D:getcontentlength></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
</D:multistatus>"#;
    let entries = parse_multistatus(body);
    assert_eq!(entries.len(), 2);
    assert!(entries[0].is_dir);
    assert_eq!(entries[0].name, "cache");
    assert!(!entries[1].is_dir);
    assert_eq!(entries[1].name, "abcdef");
    assert_eq!(entries[1].size, 123);
}
