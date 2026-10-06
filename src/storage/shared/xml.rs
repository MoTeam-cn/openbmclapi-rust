//! Minimal XML helpers shared by the S3 and OSS listing parsers.

/// One object returned by a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListedObject {
    pub(crate) key: String,
    pub(crate) size: i64,
}

/// Collect the raw text of every `<tag>...</tag>` block.
pub(crate) fn xml_blocks(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut blocks = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        blocks.push(after[..end].to_string());
        rest = &after[end + close.len()..];
    }
    blocks
}

/// Text of the first `<tag>...</tag>` element, with entities decoded.
pub(crate) fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(decode_entities(&xml[start..end]))
}

/// Decode the five XML entities plus numeric character references.
fn decode_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        if let Some(semi) = rest.find(';') {
            if semi <= 12 {
                if let Some(decoded) = decode_entity(&rest[1..semi]) {
                    out.push(decoded);
                    rest = &rest[semi + 1..];
                    continue;
                }
            }
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

/// Decode one entity body (without the leading `&` and trailing `;`).
fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            if let Some(hex) = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
            {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else {
                entity
                    .strip_prefix('#')
                    .and_then(|dec| dec.parse::<u32>().ok())
                    .and_then(char::from_u32)
            }
        }
    }
}

#[cfg(test)]
#[path = "xml_test.rs"]
mod tests;
