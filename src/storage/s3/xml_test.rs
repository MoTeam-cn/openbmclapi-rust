use super::{parse_s3_list, ListedObject};

#[test]
fn parse_list_extracts_objects() {
    let xml = r#"<?xml version="1.0"?><ListBucketResult><IsTruncated>true</IsTruncated><Contents><Key>ab/abc&amp;d</Key><Size>12</Size></Contents><NextContinuationToken>tok</NextContinuationToken></ListBucketResult>"#;
    let page = parse_s3_list(xml);
    assert!(page.is_truncated);
    assert_eq!(page.next_token.as_deref(), Some("tok"));
    assert_eq!(
        page.objects,
        vec![ListedObject {
            key: "ab/abc&d".to_string(),
            size: 12
        }]
    );
}

#[test]
fn parse_list_without_truncation() {
    let xml = r#"<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>ab/one</Key><Size>1</Size></Contents></ListBucketResult>"#;
    let page = parse_s3_list(xml);
    assert!(!page.is_truncated);
    assert!(page.next_token.is_none());
    assert_eq!(page.objects.len(), 1);
}
