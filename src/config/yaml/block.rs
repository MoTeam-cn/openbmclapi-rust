//! Block mappings and sequences, and the entry point of the reader.

use serde_json::{Map, Value};

use super::error::YamlError;
use super::flow;
use super::parser::Parser;
use super::scalar;

/// Parse a YAML document into a JSON value.
///
/// A document with no content at all becomes null, mirroring an empty file.
pub(crate) fn parse(input: &str) -> Result<Value, YamlError> {
    let mut parser = Parser::new(input);
    if !parser.skip_to_content()? {
        return Ok(Value::Null);
    }
    let indent = parser.column();
    let value = if parser.is_sequence_entry() {
        sequence(&mut parser, indent)?
    } else {
        mapping(&mut parser, indent)?
    };
    if parser.skip_to_content()? {
        return Err(parser.error("unexpected trailing content"));
    }
    Ok(value)
}

/// Parse a nested block whose first line must be indented past min_indent.
///
/// Leaves the cursor untouched and returns null when the next content line
/// belongs to an outer block.
fn block(parser: &mut Parser, min_indent: usize) -> Result<Value, YamlError> {
    let mark = parser.mark();
    if !parser.skip_to_content()? {
        return Ok(Value::Null);
    }
    let indent = parser.column();
    if indent < min_indent {
        parser.reset(mark);
        return Ok(Value::Null);
    }
    if parser.is_sequence_entry() {
        sequence(parser, indent)
    } else {
        mapping(parser, indent)
    }
}

fn mapping(parser: &mut Parser, indent: usize) -> Result<Value, YamlError> {
    let mut map = Map::new();
    loop {
        if !parser.skip_to_content()? {
            break;
        }
        let column = parser.column();
        if column < indent {
            break;
        }
        if column > indent {
            return Err(parser.error("unexpected indentation"));
        }
        if parser.is_sequence_entry() {
            return Err(parser.error("unexpected sequence entry inside a mapping"));
        }
        let key = key(parser)?;
        if parser.peek() != Some(':') {
            return Err(parser.error("expected ':' after a mapping key"));
        }
        parser.bump();
        parser.skip_inline_spaces();
        let value = if parser.at_line_end() {
            // A key with nothing after the colon opens a block; an empty one
            // stays an empty mapping rather than null.
            let nested = block(parser, indent + 1)?;
            if nested.is_null() {
                Value::Object(Map::new())
            } else {
                nested
            }
        } else {
            inline(parser)?
        };
        if map.insert(key.clone(), value).is_some() {
            return Err(parser.error(format!("duplicate key {key:?}")));
        }
    }
    Ok(Value::Object(map))
}

fn sequence(parser: &mut Parser, indent: usize) -> Result<Value, YamlError> {
    let mut items = Vec::new();
    loop {
        if !parser.skip_to_content()? {
            break;
        }
        let column = parser.column();
        if column < indent {
            break;
        }
        if column > indent {
            return Err(parser.error("unexpected indentation"));
        }
        if !parser.is_sequence_entry() {
            return Err(parser.error("expected a sequence entry"));
        }
        parser.bump();
        parser.skip_inline_spaces();
        if parser.at_line_end() {
            items.push(block(parser, indent + 1)?);
            continue;
        }
        let item_indent = parser.column();
        if looks_like_mapping(parser) {
            items.push(mapping(parser, item_indent)?);
        } else {
            let value = inline(parser)?;
            parser.skip_inline_spaces();
            if !parser.at_line_end() {
                return Err(parser.error("unexpected trailing content after a sequence entry"));
            }
            items.push(value);
        }
    }
    Ok(Value::Array(items))
}

/// Parse a value that starts on the current line.
fn inline(parser: &mut Parser) -> Result<Value, YamlError> {
    match parser.peek() {
        Some('[') | Some('{') => flow::collection(parser),
        Some('"') | Some('\'') => scalar::quoted(parser).map(Value::String),
        Some('&') => Err(parser.error("anchors (&) are not supported")),
        Some('*') => Err(parser.error("aliases (*) are not supported")),
        Some('!') => Err(parser.error("tags (!) are not supported")),
        Some('|') | Some('>') => {
            Err(parser.error("multi-line scalars (| and >) are not supported"))
        }
        _ => scalar::plain(parser),
    }
}

/// Read a mapping key; rejects the constructs this reader does not support.
fn key(parser: &mut Parser) -> Result<String, YamlError> {
    let line = parser.line();
    if matches!(parser.peek(), Some('"') | Some('\'')) {
        let key = scalar::quoted(parser)?;
        scalar::reject_key_indicator(&key, line)?;
        return Ok(key);
    }
    let mut raw = String::new();
    while let Some(c) = parser.peek() {
        if matches!(c, '\n' | '\r') {
            break;
        }
        if c == ':' && key_ends_here(parser) {
            break;
        }
        if c == '#' && parser.comment_starts_here() {
            break;
        }
        raw.push(c);
        parser.bump();
    }
    let key = raw.trim();
    if key.is_empty() {
        return Err(parser.error("expected a mapping key"));
    }
    scalar::reject_key_indicator(key, line)?;
    Ok(key.to_string())
}

/// True when the colon under the cursor separates a key from its value.
fn key_ends_here(parser: &Parser) -> bool {
    matches!(
        parser.peek_at(1),
        None | Some(' ') | Some('\t') | Some('\n') | Some('\r')
    )
}

/// True when the current line opens key: value, i.e. a mapping sequence entry.
fn looks_like_mapping(parser: &Parser) -> bool {
    let first = match parser.peek_at(0) {
        None | Some('[') | Some('{') => return false,
        Some(c) => c,
    };
    let mut offset = 0;
    if first == '"' || first == '\'' {
        offset = 1;
        loop {
            match parser.peek_at(offset) {
                None | Some('\n') => return false,
                Some(c) if c == first => {
                    offset += 1;
                    break;
                }
                Some('\\') if first == '"' => offset += 2,
                _ => offset += 1,
            }
        }
    } else {
        loop {
            match parser.peek_at(offset) {
                None | Some('\n') | Some('\r') => return false,
                Some('#') => {
                    let previous = offset.checked_sub(1).and_then(|i| parser.peek_at(i));
                    if matches!(previous, None | Some(' ') | Some('\t')) {
                        return false;
                    }
                    offset += 1;
                }
                Some(':') => {
                    return matches!(
                        parser.peek_at(offset + 1),
                        None | Some(' ') | Some('\t') | Some('\n') | Some('\r')
                    );
                }
                _ => offset += 1,
            }
        }
    }
    while matches!(parser.peek_at(offset), Some(' ') | Some('\t')) {
        offset += 1;
    }
    parser.peek_at(offset) == Some(':')
}
