use super::*;

#[test]
fn parse_list_pages() {
    let xml = r#"<ListBucketResult><IsTruncated>true</IsTruncated><NextMarker>ab/next</NextMarker><Contents><Key>ab/one</Key><Size>1</Size></Contents><Contents><Key>ab/two</Key><Size>2</Size></Contents></ListBucketResult>"#;
    let page = parse_oss_list(xml);
    assert!(page.is_truncated);
    assert_eq!(page.next_marker.as_deref(), Some("ab/next"));
    assert_eq!(page.objects.len(), 2);
    assert_eq!(page.objects[1].size, 2);
    assert_eq!(page.objects[0].key, "ab/one");
}

#[test]
fn parse_list_self_closing_marker() {
    let xml = r#"<ListBucketResult><IsTruncated>false</IsTruncated><NextMarker/><Contents><Key>ab/one</Key><Size>1</Size></Contents></ListBucketResult>"#;
    let page = parse_oss_list(xml);
    assert!(!page.is_truncated);
    assert!(page.next_marker.is_none());
    assert_eq!(page.objects.len(), 1);
}
