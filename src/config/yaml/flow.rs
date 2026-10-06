//! Flow collections: [a, b] and {a: 1, b: 2}, nested to any depth.

use serde_json::{Map, Value};

use super::error::YamlError;
use super::parser::Parser;
use super::scalar;

/// Parse a flow collection; the cursor must sit on an opening bracket.
pub(super) fn collection(parser: &mut Parser) -> Result<Value, YamlError> {
    match parser.bump() {
        Some('[') => sequence(parser),
        Some('{') => mapping(parser),
        _ => Err(parser.error("expected a flow collection")),
    }
}

/// Parse one flow value; a missing value such as [a, , b] becomes null.
pub(super) fn item(parser: &mut Parser) -> Result<Value, YamlError> {
    match parser.peek() {
        Some('[') | Some('{') => collection(parser),
        Some('"') | Some('\'') => scalar::quoted(parser).map(Value::String),
        Some(',') | Some(']') | Some('}') => Ok(Value::Null),
        Some('&') => Err(parser.error("anchors (&) are not supported")),
        Some('*') => Err(parser.error("aliases (*) are not supported")),
        Some('!') => Err(parser.error("tags (!) are not supported")),
        Some('|') | Some('>') => {
            Err(parser.error("multi-line scalars (| and >) are not supported"))
        }
        None => Err(parser.error("unterminated flow collection")),
        _ => scalar::flow_plain(parser),
    }
}

fn sequence(parser: &mut Parser) -> Result<Value, YamlError> {
    let mut items = Vec::new();
    loop {
        parser.skip_flow_ws();
        match parser.peek() {
            Some(']') => {
                parser.bump();
                return Ok(Value::Array(items));
            }
            None => return Err(parser.error("unterminated flow sequence")),
            _ => {}
        }
        items.push(item(parser)?);
        parser.skip_flow_ws();
        match parser.peek() {
            Some(',') => {
                parser.bump();
            }
            Some(']') => {
                parser.bump();
                return Ok(Value::Array(items));
            }
            None => return Err(parser.error("unterminated flow sequence")),
            Some(c) => return Err(parser.error(format!("unexpected {c:?} in a flow sequence"))),
        }
    }
}

fn mapping(parser: &mut Parser) -> Result<Value, YamlError> {
    let mut map = Map::new();
    loop {
        parser.skip_flow_ws();
        match parser.peek() {
            Some('}') => {
                parser.bump();
                return Ok(Value::Object(map));
            }
            None => return Err(parser.error("unterminated flow mapping")),
            _ => {}
        }
        let key = key(parser)?;
        parser.skip_flow_ws();
        if parser.peek() != Some(':') {
            return Err(parser.error("expected ':' in a flow mapping"));
        }
        parser.bump();
        parser.skip_flow_ws();
        let value = item(parser)?;
        if map.insert(key.clone(), value).is_some() {
            return Err(parser.error(format!("duplicate key {key:?}")));
        }
        parser.skip_flow_ws();
        match parser.peek() {
            Some(',') => {
                parser.bump();
            }
            Some('}') => {
                parser.bump();
                return Ok(Value::Object(map));
            }
            None => return Err(parser.error("unterminated flow mapping")),
            Some(c) => return Err(parser.error(format!("unexpected {c:?} in a flow mapping"))),
        }
    }
}

fn key(parser: &mut Parser) -> Result<String, YamlError> {
    let line = parser.line();
    if matches!(parser.peek(), Some('"') | Some('\'')) {
        let key = scalar::quoted(parser)?;
        scalar::reject_key_indicator(&key, line)?;
        return Ok(key);
    }
    let mut raw = String::new();
    while let Some(c) = parser.peek() {
        if matches!(c, '\n' | '\r' | ':' | ',' | '}') {
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
        return Err(parser.error("empty key in a flow mapping"));
    }
    scalar::reject_key_indicator(key, line)?;
    Ok(key.to_string())
}
