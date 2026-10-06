use super::{xml_blocks, xml_tag};

#[test]
fn xml_blocks_collects_each_block() {
    let xml = "<Root><Contents><Key>a</Key></Contents><Contents><Key>b</Key></Contents></Root>";
    assert_eq!(
        xml_blocks(xml, "Contents"),
        vec!["<Key>a</Key>".to_string(), "<Key>b</Key>".to_string()]
    );
}

#[test]
fn xml_blocks_stops_at_unclosed_block() {
    assert!(xml_blocks("<Contents><Key>a</Key>", "Contents").is_empty());
}

#[test]
fn xml_tag_decodes_entities() {
    let xml = "<Contents><Key>ab/abc&amp;d</Key></Contents>";
    assert_eq!(xml_tag(xml, "Key").as_deref(), Some("ab/abc&d"));
}

#[test]
fn xml_tag_decodes_numeric_references() {
    assert_eq!(xml_tag("<K>&#65;&#x42;</K>", "K").as_deref(), Some("AB"));
}

#[test]
fn xml_tag_missing_is_none() {
    assert_eq!(xml_tag("<Root/>", "Key"), None);
}
