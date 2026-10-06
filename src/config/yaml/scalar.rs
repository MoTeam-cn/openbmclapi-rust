//! Scalar scanning: plain, single quoted and double quoted, plus type inference.

use serde_json::{Number, Value};

use super::error::YamlError;
use super::parser::Parser;

/// Read a quoted scalar; the cursor must sit on the opening quote.
pub(super) fn quoted(parser: &mut Parser) -> Result<String, YamlError> {
    let start = parser.line();
    let Some(quote) = parser.bump() else {
        return Err(parser.error("expected a quoted scalar"));
    };
    let mut out = String::new();
    loop {
        let Some(c) = parser.bump() else {
            return Err(YamlError::new(start, "unterminated quoted scalar"));
        };
        if c == '\n' {
            return Err(YamlError::new(start, "quoted scalars cannot span lines"));
        }
        if c == quote {
            // A doubled single quote is the only escape in a single-quoted scalar.
            if quote == '\'' && parser.peek() == Some('\'') {
                parser.bump();
                out.push('\'');
                continue;
            }
            return Ok(out);
        }
        if c == '\\' && quote == '"' {
            out.push(escape(parser, start)?);
            continue;
        }
        out.push(c);
    }
}

/// Read a plain scalar up to the end of the line or a trailing comment.
pub(super) fn plain(parser: &mut Parser) -> Result<Value, YamlError> {
    let mut raw = String::new();
    while let Some(c) = parser.peek() {
        if c == '\n' || c == '\r' {
            break;
        }
        if c == '#' && parser.comment_starts_here() {
            break;
        }
        raw.push(c);
        parser.bump();
    }
    Ok(infer(raw.trim()))
}

/// Read a plain scalar inside a flow collection, stopping at a delimiter.
pub(super) fn flow_plain(parser: &mut Parser) -> Result<Value, YamlError> {
    let mut raw = String::new();
    while let Some(c) = parser.peek() {
        if matches!(c, '\n' | '\r' | ',' | ']' | '}') {
            break;
        }
        if c == '#' && parser.comment_starts_here() {
            break;
        }
        raw.push(c);
        parser.bump();
    }
    Ok(infer(raw.trim()))
}

/// Reject keys that name an unsupported YAML construct.
pub(super) fn reject_key_indicator(key: &str, line: usize) -> Result<(), YamlError> {
    let message = match key.chars().next() {
        Some('&') => Some("anchors (&) are not supported"),
        Some('*') => Some("aliases (*) are not supported"),
        Some('!') => Some("tags (!) are not supported"),
        _ if key == "<<" => Some("merge keys (<<) are not supported"),
        _ => None,
    };
    match message {
        Some(message) => Err(YamlError::new(line, message)),
        None => Ok(()),
    }
}

/// Classify a plain scalar the way the YAML core schema does.
pub(super) fn infer(text: &str) -> Value {
    if text.is_empty() || text == "~" || text.eq_ignore_ascii_case("null") {
        return Value::Null;
    }
    if text.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if text.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if let Some(number) = integer(text) {
        return Value::Number(number);
    }
    if let Some(number) = float(text) {
        return Value::Number(number);
    }
    Value::String(text.to_string())
}

fn integer(text: &str) -> Option<Number> {
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>()
        .map(Number::from)
        .or_else(|_| text.parse::<u64>().map(Number::from))
        .ok()
}

fn float(text: &str) -> Option<Number> {
    let body = text.strip_prefix(['-', '+']).unwrap_or(text);
    if !body.contains('.') && !body.contains('e') && !body.contains('E') {
        return None;
    }
    text.parse::<f64>().ok().and_then(Number::from_f64)
}

fn escape(parser: &mut Parser, start: usize) -> Result<char, YamlError> {
    let Some(c) = parser.bump() else {
        return Err(YamlError::new(start, "unterminated escape sequence"));
    };
    Ok(match c {
        '\\' => '\\',
        '"' => '"',
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        'u' => unicode_escape(parser, start)?,
        other => return Err(parser.error(format!("unsupported escape sequence \\{other}"))),
    })
}

fn unicode_escape(parser: &mut Parser, start: usize) -> Result<char, YamlError> {
    let mut code = 0u32;
    for _ in 0..4 {
        let Some(c) = parser.bump() else {
            return Err(YamlError::new(start, "truncated \\u escape"));
        };
        let Some(digit) = c.to_digit(16) else {
            return Err(YamlError::new(
                start,
                format!("invalid hex digit {c:?} in a \\u escape"),
            ));
        };
        code = code * 16 + digit;
    }
    char::from_u32(code).ok_or_else(|| YamlError::new(start, "\\u escape is not a valid character"))
}
