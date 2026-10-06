//! Parsing of the OSS `ListObjects` XML response.

use crate::storage::shared::{xml_blocks, xml_tag, ListedObject};

/// One page of an OSS `ListObjects` response.
#[derive(Debug, Default)]
pub(super) struct OssListPage {
    pub(super) objects: Vec<ListedObject>,
    pub(super) is_truncated: bool,
    pub(super) next_marker: Option<String>,
}

/// Parse the subset of OSS `ListObjects` XML the backend needs.
pub(super) fn parse_oss_list(xml: &str) -> OssListPage {
    let objects = xml_blocks(xml, "Contents")
        .into_iter()
        .filter_map(|block| {
            let key = xml_tag(&block, "Key")?;
            let size = xml_tag(&block, "Size")
                .and_then(|value| value.trim().parse::<i64>().ok())
                .unwrap_or(0);
            Some(ListedObject { key, size })
        })
        .collect();
    let is_truncated = xml_tag(xml, "IsTruncated")
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let next_marker = xml_tag(xml, "NextMarker").filter(|value| !value.is_empty());
    OssListPage {
        objects,
        is_truncated,
        next_marker,
    }
}

#[cfg(test)]
#[path = "xml_test.rs"]
mod tests;
