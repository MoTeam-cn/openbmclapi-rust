//! Public socket.io handle and the state shared with the background task.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Notify};

use crate::error::{Error, Result};
use crate::token::TokenManager;

use super::events::{Command, SocketEvent};
use super::protocol::websocket_url;

/// Pending acknowledgement senders keyed by request id.
pub(super) type AckMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// A connected (or reconnecting) socket.io client.
#[derive(Clone)]
pub struct SocketIo {
    inner: Arc<Inner>,
}

/// State shared between the public handle and the background socket task.
pub(super) struct Inner {
    pub(super) url: String,
    pub(super) token: TokenManager,
    commands: mpsc::UnboundedSender<Command>,
    commands_rx: Mutex<Option<mpsc::UnboundedReceiver<Command>>>,
    pub(super) events: broadcast::Sender<SocketEvent>,
    pub(super) acks: AckMap,
    next_id: AtomicU64,
    started: AtomicBool,
    pub(super) closed: AtomicBool,
    pub(super) connected: AtomicBool,
    pub(super) wake: Notify,
}

impl SocketIo {
    /// Create a client for the master base URL; the socket starts on `connect`.
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

    /// Report whether the transport is currently connected.
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
