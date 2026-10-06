//! `207 Multi-Status` parsing for WebDAV directory listings.

use quick_xml::events::Event;
use quick_xml::Reader;
use tracing::warn;

/// A single entry of a WebDAV directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavEntry {
    /// Fully qualified URL of the entry.
    pub href: String,
    /// Last path segment, used as the content hash.
    pub name: String,
    /// Object size in bytes; zero for collections.
    pub size: i64,
    /// Whether the entry is a collection (directory).
    pub is_dir: bool,
}

/// Parse a `207 Multi-Status` document into flat entries.
pub fn parse_multistatus(body: &str) -> Vec<DavEntry> {
    let mut reader = Reader::from_str(body);
    let mut entries = Vec::new();
    let mut current: Option<DavEntry> = None;
    let mut text_target: Option<&'static str> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let local = local_name(event.name().as_ref());
                match local.as_str() {
                    "response" => {
                        current = Some(DavEntry {
                            href: String::new(),
                            name: String::new(),
                            size: 0,
                            is_dir: false,
                        });
                    }
                    "href" => text_target = Some("href"),
                    "getcontentlength" => text_target = Some("size"),
                    _ => {}
                }
            }
            Ok(Event::Empty(event)) => {
                if local_name(event.name().as_ref()) == "collection" {
                    if let Some(entry) = current.as_mut() {
                        entry.is_dir = true;
                    }
                }
            }
            Ok(Event::Text(event)) => {
                if let (Some(entry), Some(target)) = (current.as_mut(), text_target) {
                    let text = String::from_utf8_lossy(event.as_ref()).trim().to_string();
                    match target {
                        "href" => entry.href = text,
                        "size" => entry.size = text.parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(event)) => {
                let local = local_name(event.name().as_ref());
                if local == "href" || local == "getcontentlength" {
                    text_target = None;
                } else if local == "response" {
                    if let Some(mut entry) = current.take() {
                        if !entry.href.is_empty() {
                            let trimmed = entry.href.trim_end_matches('/');
                            entry.name = trimmed.rsplit('/').next().unwrap_or(trimmed).to_string();
                            entries.push(entry);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!(error = %e, "failed to parse WebDAV multistatus");
                break;
            }
            _ => {}
        }
    }
    entries
}

fn local_name(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    match text.rsplit_once(':') {
        Some((_, local)) => local.to_ascii_lowercase(),
        None => text.to_ascii_lowercase(),
    }
}

#[cfg(test)]
#[path = "xml_test.rs"]
mod tests;
