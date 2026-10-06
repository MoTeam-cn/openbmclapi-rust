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

mod client;
mod events;
mod protocol;
mod session;

pub use client::SocketIo;
pub use events::SocketEvent;
pub use protocol::websocket_url;
