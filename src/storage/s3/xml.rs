//! `ListObjectsV2` response parsing.

use crate::storage::shared::{xml_blocks, xml_tag, ListedObject};

/// One page of a `ListObjectsV2` response.
#[derive(Debug, Default)]
pub(super) struct S3ListPage {
    pub(super) objects: Vec<ListedObject>,
    pub(super) is_truncated: bool,
    pub(super) next_token: Option<String>,
}

/// Parse the subset of `ListObjectsV2` XML the backend needs.
pub(super) fn parse_s3_list(xml: &str) -> S3ListPage {
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
    let next_token = xml_tag(xml, "NextContinuationToken").filter(|value| !value.is_empty());
    S3ListPage {
        objects,
        is_truncated,
        next_token,
    }
}

#[cfg(test)]
#[path = "xml_test.rs"]
mod tests;
