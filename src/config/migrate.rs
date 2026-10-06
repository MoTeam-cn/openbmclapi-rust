//! Conversion of a Node-agent `.env` file into our YAML configuration.
//!
//! The Node agent read the environment only, so an operator moving over has a
//! `.env` and nothing else. Every key it understands is mapped onto the YAML key
//! carrying the same meaning. A key with no counterpart is reported rather than
//! dropped: a setting that silently disappears is worse than a loud one.
//!
//! Storage options stay in JSON. YAML is a superset of JSON, the reader accepts
//! flow mappings, and the Node value is already JSON — so there is nothing to
//! translate and no YAML writer to write.

use serde_json::Value;

use crate::error::{Error, Result};

use super::dotenv;

/// How one environment variable is rendered.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Kind {
    /// A double-quoted string.
    Text,
    /// `true` or `false`.
    Bool,
    /// A bare integer.
    Number,
    /// A flow list of integers, from a comma-separated value.
    Sizes,
}

/// One Node variable and the YAML key it becomes.
struct Mapping {
    env: &'static str,
    yaml: &'static str,
    kind: Kind,
}

const MAPPINGS: &[Mapping] = &[
    Mapping {
        env: "CLUSTER_ID",
        yaml: "cluster_id",
        kind: Kind::Text,
    },
    Mapping {
        env: "CLUSTER_SECRET",
        yaml: "cluster_secret",
        kind: Kind::Text,
    },
    Mapping {
        env: "CLUSTER_IP",
        yaml: "cluster_ip",
        kind: Kind::Text,
    },
    Mapping {
        env: "CLUSTER_PORT",
        yaml: "port",
        kind: Kind::Number,
    },
    Mapping {
        env: "CLUSTER_PUBLIC_PORT",
        yaml: "cluster_public_port",
        kind: Kind::Number,
    },
    Mapping {
        env: "CLUSTER_BYOC",
        yaml: "byoc",
        kind: Kind::Bool,
    },
    Mapping {
        env: "CLUSTER_BMCLAPI",
        yaml: "bmclapi_base",
        kind: Kind::Text,
    },
    Mapping {
        env: "SSL_KEY",
        yaml: "ssl_key",
        kind: Kind::Text,
    },
    Mapping {
        env: "SSL_CERT",
        yaml: "ssl_cert",
        kind: Kind::Text,
    },
    Mapping {
        env: "ENABLE_UPNP",
        yaml: "enable_upnp",
        kind: Kind::Bool,
    },
    Mapping {
        env: "DISABLE_ACCESS_LOG",
        yaml: "disable_access_log",
        kind: Kind::Bool,
    },
    Mapping {
        env: "DISABLE_SIGN",
        yaml: "disable_sign",
        kind: Kind::Bool,
    },
    Mapping {
        env: "NO_DAEMON",
        yaml: "no_daemon",
        kind: Kind::Bool,
    },
    Mapping {
        env: "NO_FAST_ENABLE",
        yaml: "no_fast_enable",
        kind: Kind::Bool,
    },
    Mapping {
        env: "SYNC_MEMORY_BUDGET",
        yaml: "sync_memory_budget",
        kind: Kind::Number,
    },
    Mapping {
        env: "MEASURE_SIZES",
        yaml: "measure_sizes",
        kind: Kind::Sizes,
    },
    Mapping {
        env: "MEASURE_REDIRECT",
        yaml: "measure_redirect",
        kind: Kind::Bool,
    },
    Mapping {
        env: "LOGLEVEL",
        yaml: "log_level",
        kind: Kind::Text,
    },
    Mapping {
        env: "LOG_FORMAT",
        yaml: "log_format",
        kind: Kind::Text,
    },
    Mapping {
        env: "LOG_DIR",
        yaml: "log_dir",
        kind: Kind::Text,
    },
    Mapping {
        env: "PLAIN_LOG",
        yaml: "plain_log",
        kind: Kind::Bool,
    },
];

/// The two variables that describe storage, handled together.
const STORAGE_TYPE: &str = "CLUSTER_STORAGE";
const STORAGE_OPTIONS: &str = "CLUSTER_STORAGE_OPTIONS";

/// Convert a Node-agent `.env` document into our YAML configuration.
pub fn convert(text: &str, source: &str) -> Result<String> {
    let pairs = dotenv::parse(text);
    let mut out = String::new();
    out.push_str(&format!("# 由 Node 版的 .env 转换而来，来源：{source}\n"));
    out.push_str("# 这里的每一项都覆盖同名的环境变量，可以按需删改。\n\n");

    let mut skipped: Vec<&str> = Vec::new();
    for (key, value) in &pairs {
        if key == STORAGE_TYPE || key == STORAGE_OPTIONS {
            continue;
        }
        match MAPPINGS.iter().find(|m| m.env == key) {
            Some(m) => out.push_str(&format!("{}: {}\n", m.yaml, scalar(&m.kind, key, value)?)),
            None => skipped.push(key),
        }
    }

    out.push('\n');
    out.push_str(&storage_block(&pairs)?);

    if !skipped.is_empty() {
        out.push('\n');
        out.push_str("# 以下键在这个程序里没有对应项，已忽略：\n");
        for key in skipped {
            out.push_str(&format!("#   {key}\n"));
        }
    }
    Ok(out)
}

/// Render the `storage` block from the two storage variables.
fn storage_block(pairs: &[(String, String)]) -> Result<String> {
    let find = |name: &str| {
        pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    let Some(kind) = find(STORAGE_TYPE) else {
        return Ok(
            "# 没有 CLUSTER_STORAGE，未写出存储配置，程序会使用它自己的缺省值。\n".to_string(),
        );
    };
    let mut out = String::from("storage:\n");
    out.push_str(&format!("  type: {}\n", quote(&kind)));
    match find(STORAGE_OPTIONS) {
        Some(raw) => {
            // Re-serialise so a hand-edited value still comes out as valid JSON,
            // which is also valid YAML.
            let parsed: Value = serde_json::from_str(&raw)
                .map_err(|e| Error::Config(format!("{STORAGE_OPTIONS} 不是合法 JSON：{e}")))?;
            out.push_str(&format!("  options: {parsed}\n"));
        }
        None => out.push_str("  # options: {}\n"),
    }
    Ok(out)
}

/// Render one value according to the type its YAML key expects.
fn scalar(kind: &Kind, key: &str, value: &str) -> Result<String> {
    let trimmed = value.trim();
    Ok(match kind {
        Kind::Text => quote(trimmed),
        Kind::Bool => if is_true(trimmed) { "true" } else { "false" }.to_string(),
        Kind::Number => {
            if trimmed.parse::<u64>().is_err() {
                return Err(Error::Config(format!(
                    "{key} 需要一个整数，实际是 {trimmed:?}"
                )));
            }
            trimmed.to_string()
        }
        Kind::Sizes => {
            let mut parts = Vec::new();
            for part in trimmed.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                if part.parse::<u64>().is_err() {
                    return Err(Error::Config(format!("{key} 里的 {part:?} 不是整数")));
                }
                parts.push(part.to_string());
            }
            format!("[{}]", parts.join(", "))
        }
    })
}

/// Read a double-quoted YAML string. Only `\` and `\"` need escaping here; a
/// value with a literal newline is not something a `.env` line can hold.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The same set of values dotenv accepts for a boolean.
fn is_true(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

#[cfg(test)]
#[path = "migrate_test.rs"]
mod tests;
