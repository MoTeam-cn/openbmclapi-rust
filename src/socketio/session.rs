//! Background connection task: handshake, frame pump and reconnection.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, trace, warn};

use crate::error::{Error, Result};

use super::client::Inner;
use super::events::{Command, SocketEvent};
use super::protocol::{reconnect_delay, split_id};

/// Transport stream carried through the whole connection lifecycle.
type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

impl Inner {
    pub(super) async fn run(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<Command>) {
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
                    error!(error = %e, "socket 连接错误");
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
                "正在重连 socket"
            );
            tokio::time::sleep(delay).await;
        }
    }

    /// Open the websocket and complete the engine.io + socket.io handshake.
    async fn open(&self) -> Result<WsStream> {
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
        debug!("socket.io 已连接");
        let _ = self.events.send(SocketEvent::Connected);
        Ok(stream)
    }

    /// Pump messages until the connection drops.
    async fn session(
        self: &Arc<Self>,
        mut stream: WsStream,
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
                            trace!("忽略二进制 socket.io 帧");
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
        info!(reason = %reason, "socket 已断开");
        let _ = self.events.send(SocketEvent::Disconnected(reason));
    }

    /// Handle one engine.io/socket.io text frame. Returns a reason on close.
    async fn handle_text(self: &Arc<Self>, stream: &mut WsStream, text: &str) -> Option<String> {
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
                // DISCONNECT: the master ended the session. Newer servers append
                // a reason to the packet, and dropping it is why a kick used to
                // read as a generic "server disconnect".
                self.connected.store(false, Ordering::Relaxed);
                let reason = match body.trim() {
                    "" => "server disconnect".to_string(),
                    other => format!("server disconnect: {other}"),
                };
                let _ = self.events.send(SocketEvent::Disconnected(reason));
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
                            info!(payload = %payload, "主控消息");
                            let _ = self.events.send(SocketEvent::Message(payload));
                        }
                        "warden-error" => {
                            warn!(payload = %payload, "主控报告了 warden 错误");
                            let _ = self.events.send(SocketEvent::WardenError(payload));
                        }
                        "exception" => {
                            error!(payload = %payload, "服务端异常");
                            let _ = self.events.send(SocketEvent::Exception(payload));
                        }
                        other => {
                            debug!(event = other, payload = %payload, "未处理的 socket 事件");
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
                // CONNECT_ERROR: the master refused the session outright, which
                // is what an unknown cluster or a kicked node looks like.
                let payload =
                    serde_json::from_str::<Value>(body).unwrap_or(Value::String(body.to_string()));
                let _ = self.events.send(SocketEvent::ConnectError(payload));
            }
            _ => {}
        }
    }

    async fn send_event(
        &self,
        stream: &mut WsStream,
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

async fn next_text(stream: &mut WsStream) -> Result<String> {
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
