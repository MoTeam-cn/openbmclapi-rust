//! Minimal engine.io v4 / socket.io v4 client.
//!
//! The Node agent talks to the master with `socket.io-client` over the
//! websocket transport only. This module implements just the subset the agent
//! uses — connect with an auth payload, emit with acknowledgement, receive
//! server events, and automatic reconnection — directly on `tokio-tungstenite`.
//!
//! Packet grammar (engine.io type + socket.io type + optional namespace + id):
//!
//! ```text
//! 0{"sid":...}            engine.io open
//! 2 / 3                   engine.io ping / pong
//! 40{"token":"..."}       socket.io connect with auth
//! 40{"sid":...}           socket.io connect ack
//! 42["event",{...}]       server -> client event
//! 42<id>["event",{...}]   client -> server event requesting an ack
//! 43<id>[null,true]       ack payload
//! 44{"message":"..."}     connect error
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Notify};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, trace, warn};

use crate::error::{Error, Result};
use crate::token::TokenManager;

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

enum Command {
    Emit { id: u64, event: String, data: Value },
    Close,
}

type AckMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// A connected (or reconnecting) socket.io client.
#[derive(Clone)]
pub struct SocketIo {
    inner: Arc<Inner>,
}

struct Inner {
    url: String,
    token: TokenManager,
    commands: mpsc::UnboundedSender<Command>,
    commands_rx: Mutex<Option<mpsc::UnboundedReceiver<Command>>>,
    events: broadcast::Sender<SocketEvent>,
    acks: AckMap,
    next_id: AtomicU64,
    started: AtomicBool,
    closed: AtomicBool,
    connected: AtomicBool,
    wake: Notify,
}

impl SocketIo {
    pub fn new(base: &str, token: TokenManager) -> Self {
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(256);
        SocketIo {
            inner: Arc::new(Inner {
                url: websocket_url(base),
                token,
                commands,
                commands_rx: Mutex::new(Some(commands_rx)),
                events,
                acks: Arc::new(Mutex::new(HashMap::new())),
                next_id: AtomicU64::new(1),
                started: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                connected: AtomicBool::new(false),
                wake: Notify::new(),
            }),
        }
    }

    /// Start (or resume) the background connection task.
    ///
    /// Safe to call repeatedly: the first call spawns the task, later calls
    /// clear a previous `disconnect` and wake the task up.
    pub async fn connect(&self) {
        self.inner.closed.store(false, Ordering::SeqCst);
        if self.inner.started.swap(true, Ordering::SeqCst) {
            self.inner.wake.notify_one();
            return;
        }
        let mut guard = self.inner.commands_rx.lock().await;
        let Some(rx) = guard.take() else { return };
        let inner = self.inner.clone();
        tokio::spawn(async move { inner.run(rx).await });
    }

    /// Subscribe to connection and server events.
    pub fn subscribe(&self) -> broadcast::Receiver<SocketEvent> {
        self.inner.events.subscribe()
    }

    pub fn is_connected(&self) -> bool {
        self.inner.connected.load(Ordering::Relaxed)
    }

    /// Emit an event and wait for the acknowledgement payload.
    ///
    /// The master replies with the callback arguments array, e.g.
    /// `[null, true]`, exactly like `socket.emitWithAck` in the Node agent.
    pub async fn emit_with_ack(
        &self,
        event: &str,
        data: Value,
        timeout: Duration,
    ) -> Result<Value> {
        if self.inner.closed.load(Ordering::Relaxed) {
            return Err(Error::Socket("socket closed".into()));
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.inner.acks.lock().await.insert(id, tx);

        {
            let acks = self.inner.acks.clone();
            let event = event.to_string();
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                if let Some(tx) = acks.lock().await.remove(&id) {
                    let _ = tx.send(Err(Error::Timeout(format!("ack timeout for {event}"))));
                }
            });
        }

        self.inner
            .commands
            .send(Command::Emit {
                id,
                event: event.to_string(),
                data,
            })
            .map_err(|_| Error::Socket("socket task is gone".into()))?;

        rx.await
            .map_err(|_| Error::Socket("ack channel closed".into()))?
    }

    /// Stop the client and prevent further reconnection.
    pub async fn disconnect(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        let _ = self.inner.commands.send(Command::Close);
    }
}

impl Inner {
    async fn run(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<Command>) {
        let mut attempt: usize = 0;
        let mut pending: Vec<(u64, String, Value)> = Vec::new();

        loop {
            if self.closed.load(Ordering::Relaxed) {
                self.fail_all_acks("socket closed");
                // Park until `connect` is called again.
                self.wake.notified().await;
                attempt = 0;
                continue;
            }

            match self.open().await {
                Ok(stream) => {
                    if attempt > 0 {
                        let _ = self.events.send(SocketEvent::Reconnect(attempt));
                    }
                    attempt = 0;
                    self.session(stream, &mut rx, &mut pending).await;
                    self.connected.store(false, Ordering::Relaxed);
                    self.fail_all_acks("connection lost");
                }
                Err(e) => {
                    let _ = self.events.send(SocketEvent::ReconnectError(e.to_string()));
                    error!(error = %e, "socket connection error");
                }
            }

            if self.closed.load(Ordering::Relaxed) {
                self.fail_all_acks("socket closed");
                self.wake.notified().await;
                attempt = 0;
                continue;
            }

            attempt += 1;
            if attempt > 30 {
                let _ = self.events.send(SocketEvent::ReconnectFailed);
            }
            let delay = reconnect_delay(attempt);
            trace!(
                attempt,
                delay_ms = delay.as_millis() as u64,
                "reconnecting socket"
            );
            tokio::time::sleep(delay).await;
        }
    }

    /// Open the websocket and complete the engine.io + socket.io handshake.
    async fn open(
        &self,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        let (mut stream, _response) = tokio_tungstenite::connect_async(self.url.as_str()).await?;

        // engine.io OPEN
        let handshake = next_text(&mut stream).await?;
        if !handshake.starts_with('0') {
            return Err(Error::Socket(format!(
                "unexpected engine.io handshake: {handshake}"
            )));
        }

        // socket.io CONNECT with the auth payload.
        let token = self.token.get_token().await?;
        let payload = serde_json::json!({ "token": token });
        stream
            .send(Message::Text(format!("40{payload}").into()))
            .await
            .map_err(|e| Error::Socket(e.to_string()))?;

        // Wait for the connect ack (or a connect error).
        loop {
            let text = next_text(&mut stream).await?;
            if text.starts_with("40") {
                break;
            }
            if let Some(reason) = text.strip_prefix("44") {
                return Err(Error::Service(format!(
                    "socket.io connect rejected: {reason}"
                )));
            }
            if text == "2" {
                stream
                    .send(Message::Text("3".into()))
                    .await
                    .map_err(|e| Error::Socket(e.to_string()))?;
            }
        }

        self.connected.store(true, Ordering::Relaxed);
        debug!("socket.io connected");
        let _ = self.events.send(SocketEvent::Connected);
        Ok(stream)
    }

    /// Pump messages until the connection drops.
    async fn session(
        self: &Arc<Self>,
        mut stream: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        rx: &mut mpsc::UnboundedReceiver<Command>,
        pending: &mut Vec<(u64, String, Value)>,
    ) {
        let reason = loop {
            tokio::select! {
                incoming = stream.next() => {
                    match incoming {
                        Some(Ok(Message::Text(text))) => {
                            if let Some(reason) = self.handle_text(&mut stream, text.as_str()).await {
                                break reason;
                            }
                        }
                        Some(Ok(Message::Binary(_))) => {
                            trace!("ignoring binary socket.io frame");
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            let _ = stream.send(Message::Pong(payload)).await;
                        }
                        Some(Ok(Message::Close(frame))) => {
                            break frame.map(|f| f.reason.to_string()).unwrap_or_else(|| "closed by server".into());
                        }
                        Some(Ok(_)) => {}
                        Some(Err(e)) => break e.to_string(),
                        None => break "stream ended".to_string(),
                    }
                }
                command = rx.recv() => {
                    match command {
                        Some(Command::Emit { id, event, data }) => {
                            if !self.acks.lock().await.contains_key(&id) {
                                continue; // already timed out
                            }
                            if self.send_event(&mut stream, id, &event, &data).await.is_err() {
                                pending.push((id, event, data));
                                break "send failed".to_string();
                            }
                        }
                        Some(Command::Close) => {
                            let _ = stream.close(None).await;
                            break "client closed".to_string();
                        }
                        None => break "command channel closed".to_string(),
                    }
                }
            }
        };

        self.connected.store(false, Ordering::Relaxed);
        info!(reason = %reason, "socket disconnected");
        let _ = self.events.send(SocketEvent::Disconnected(reason));
    }

    /// Handle one engine.io/socket.io text frame. Returns a reason on close.
    async fn handle_text(
        self: &Arc<Self>,
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        text: &str,
    ) -> Option<String> {
        let (engine, rest) = text.split_at(1.min(text.len()));
        match engine {
            "0" => {
                // Re-handshake after a reconnect: re-send the socket.io CONNECT.
                if let Ok(token) = self.token.get_token().await {
                    let payload = serde_json::json!({ "token": token });
                    let _ = stream
                        .send(Message::Text(format!("40{payload}").into()))
                        .await;
                }
                None
            }
            "1" => Some("engine.io close".to_string()),
            "2" => {
                let _ = stream.send(Message::Text("3".into())).await;
                None
            }
            "3" => None,
            "4" => {
                self.handle_socketio(rest).await;
                None
            }
            _ => None,
        }
    }

    async fn handle_socketio(self: &Arc<Self>, rest: &str) {
        let Some(kind) = rest.chars().next() else {
            return;
        };
        let body = &rest[kind.len_utf8()..];
        match kind {
            '0' => {
                self.connected.store(true, Ordering::Relaxed);
            }
            '1' => {
                let _ = self
                    .events
                    .send(SocketEvent::Disconnected("server disconnect".into()));
            }
            '2' => {
                // Server-initiated event: 42["name",payload]
                let (_, data) = split_id(body);
                if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                    let (name, payload) = match &parsed {
                        Value::Array(items) if !items.is_empty() => (
                            items[0].as_str().unwrap_or_default().to_string(),
                            items.get(1).cloned().unwrap_or(Value::Null),
                        ),
                        _ => (String::new(), parsed.clone()),
                    };
                    match name.as_str() {
                        "message" => {
                            info!(payload = %payload, "master message");
                            let _ = self.events.send(SocketEvent::Message(payload));
                        }
                        "warden-error" => {
                            warn!(payload = %payload, "master reported a warden error");
                            let _ = self.events.send(SocketEvent::WardenError(payload));
                        }
                        "exception" => {
                            error!(payload = %payload, "server exception");
                            let _ = self.events.send(SocketEvent::Exception(payload));
                        }
                        other => {
                            debug!(event = other, payload = %payload, "unhandled socket event");
                        }
                    }
                }
            }
            '3' => {
                // Ack: 43<id>[args]
                let (id, data) = split_id(body);
                if let Some(id) = id {
                    if let Some(tx) = self.acks.lock().await.remove(&id) {
                        let parsed = serde_json::from_str::<Value>(data).unwrap_or(Value::Null);
                        let _ = tx.send(Ok(parsed));
                    }
                }
            }
            '4' => {
                let _ = self
                    .events
                    .send(SocketEvent::Exception(Value::String(body.to_string())));
            }
            _ => {}
        }
    }

    async fn send_event(
        &self,
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        id: u64,
        event: &str,
        data: &Value,
    ) -> Result<()> {
        let payload = serde_json::json!([event, data]);
        let frame = format!("42{id}{payload}");
        stream
            .send(Message::Text(frame.into()))
            .await
            .map_err(|e| Error::Socket(e.to_string()))
    }

    fn fail_all_acks(&self, reason: &str) {
        let acks = self.acks.clone();
        let reason = reason.to_string();
        tokio::spawn(async move {
            let mut guard = acks.lock().await;
            for (_, tx) in guard.drain() {
                let _ = tx.send(Err(Error::Socket(reason.clone())));
            }
        });
    }
}

/// Split an optional socket.io ack id from the front of a packet body.
fn split_id(body: &str) -> (Option<u64>, &str) {
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

async fn next_text(
    stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<String> {
    loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => return Ok(text.as_str().to_string()),
            Some(Ok(Message::Ping(payload))) => {
                let _ = stream.send(Message::Pong(payload)).await;
            }
            Some(Ok(Message::Close(frame))) => {
                return Err(Error::Socket(
                    frame
                        .map(|f| f.reason.to_string())
                        .unwrap_or_else(|| "closed".into()),
                ))
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(Error::Socket(e.to_string())),
            None => return Err(Error::Socket("stream ended during handshake".into())),
        }
    }
}

/// socket.io default reconnection policy: 1s doubling to 5s, +/-50% jitter.
fn reconnect_delay(attempt: usize) -> Duration {
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
mod tests {
    use super::*;

    #[test]
    fn maps_schemes() {
        assert_eq!(
            websocket_url("https://openbmclapi.bangbang93.com"),
            "wss://openbmclapi.bangbang93.com/socket.io/?EIO=4&transport=websocket"
        );
        assert_eq!(
            websocket_url("http://localhost:4000/"),
            "ws://localhost:4000/socket.io/?EIO=4&transport=websocket"
        );
    }

    #[test]
    fn splits_ack_ids() {
        assert_eq!(split_id(r#"["a",1]"#), (None, r#"["a",1]"#));
        assert_eq!(split_id(r#"7["a",1]"#), (Some(7), r#"["a",1]"#));
        assert_eq!(split_id("/admin,12[1]"), (Some(12), "[1]"));
    }
}
