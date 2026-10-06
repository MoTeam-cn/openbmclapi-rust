//! Coercing YAML values into configuration fields.
//!
//! Every key accepts the loose shapes an operator is likely to write — a port
//! as a string, a boolean as `0`/`1` — and refuses anything else with a
//! message naming the key, so a typo never lands as a silent default.

use serde_json::Value;

use crate::error::{Error, Result};
use crate::util::parse_bool;

pub(super) fn string(key: &str, value: &Value) -> Result<String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        other => Err(type_error(key, "a string", other)),
    }
}

pub(super) fn optional_string(key: &str, value: &Value) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        other => string(key, other).map(Some),
    }
}

pub(super) fn boolean(key: &str, value: &Value) -> Result<bool> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        Value::String(text) => Ok(parse_bool(text)),
        Value::Number(number) if number.as_u64() == Some(0) => Ok(false),
        Value::Number(number) if number.as_u64() == Some(1) => Ok(true),
        other => Err(type_error(key, "a boolean", other)),
    }
}

/// MiB sizes, written either as a sequence or as a comma-separated string.
pub(super) fn size_list(key: &str, value: &Value) -> Result<Vec<u64>> {
    let raw = match value {
        Value::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::Number(number) => parts.push(number.to_string()),
                    Value::String(text) => parts.push(text.clone()),
                    other => return Err(type_error(key, "a list of sizes", other)),
                }
            }
            parts.join(",")
        }
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => return Err(type_error(key, "a list of sizes", other)),
    };
    super::env::parse_size_list(&raw)
}

pub(super) fn positive(key: &str, value: &Value) -> Result<u64> {
    let number = match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| type_error(key, "a positive number", value))?,
        Value::String(text) => text
            .trim()
            .parse::<u64>()
            .map_err(|e| Error::Config(format!("{key:?} 不是数字：{e}")))?,
        other => return Err(type_error(key, "a positive number", other)),
    };
    if number == 0 {
        return Err(Error::Config(format!("{key:?} 必须大于 0")));
    }
    Ok(number)
}

pub(super) fn port(key: &str, value: &Value) -> Result<u16> {
    let number = match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| type_error(key, "a port number", value))?,
        Value::String(text) => text
            .trim()
            .parse::<u64>()
            .map_err(|e| Error::Config(format!("{key:?} 不是合法端口：{e}")))?,
        other => return Err(type_error(key, "a port number", other)),
    };
    u16::try_from(number).map_err(|_| Error::Config(format!("{key:?} 端口 {number} 超出范围")))
}

pub(super) fn type_error(key: &str, expected: &str, value: &Value) -> Error {
    Error::Config(format!("{key:?} 应为 {expected}，实际是 {}", kind(value)))
}

pub(super) fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "a mapping",
    }
}
