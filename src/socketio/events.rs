//! Events and commands exchanged between the socket task and the cluster.

use serde_json::Value;

/// Events surfaced to the cluster.
#[derive(Debug, Clone)]
pub enum SocketEvent {
    /// Transport plus socket.io handshake completed.
    Connected,
    /// Connection lost; the payload is the reason.
    Disconnected(String),
    /// A plain `message` event from the master.
    Message(Value),
    /// The master reported a warden (inspection) failure.
    WardenError(Value),
    /// A server-side exception event.
    Exception(Value),
    /// Reconnection succeeded after `attempt` failures.
    Reconnect(usize),
    /// A reconnection attempt failed.
    ReconnectError(String),
    /// Reconnection gave up.
    ReconnectFailed,
}

/// Commands sent from the public handle to the background socket task.
pub(super) enum Command {
    Emit { id: u64, event: String, data: Value },
    Close,
}
