//! Pure socket.io wire helpers: ack-id parsing, backoff and endpoint building.

use std::time::Duration;

/// Split an optional socket.io ack id from the front of a packet body.
pub(super) fn split_id(body: &str) -> (Option<u64>, &str) {
    let mut rest = body;
    if let Some(stripped) = rest.strip_prefix('/') {
        match stripped.find(',') {
            Some(pos) => rest = &stripped[pos + 1..],
            None => return (None, rest),
        }
    }
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return (None, rest);
    }
    (digits.parse().ok(), &rest[digits.len()..])
}

/// socket.io default reconnection policy: 1s doubling to 5s, +/-50% jitter.
pub(super) fn reconnect_delay(attempt: usize) -> Duration {
    let exponent = attempt.min(10) as i32;
    let base = 1000.0_f64 * 2f64.powi(exponent);
    let capped = base.min(5000.0);
    let jitter = 1.0 + (rand::random::<f64>() * 2.0 - 1.0) * 0.5;
    Duration::from_millis((capped * jitter) as u64)
}

/// Build the websocket endpoint for the given master base URL.
pub fn websocket_url(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    let scheme_mapped = if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        trimmed.to_string()
    };
    format!("{scheme_mapped}/socket.io/?EIO=4&transport=websocket")
}

#[cfg(test)]
#[path = "protocol_test.rs"]
mod tests;
