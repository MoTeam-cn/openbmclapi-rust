use std::io;

/// Crate-wide result type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("url parse error: {0}")]
    Url(#[from] url::ParseError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("socket error: {0}")]
    Socket(String),

    #[error("websocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("master returned an error: {0}")]
    Service(String),

    #[error("unexpected response status {status} for {url}")]
    Status { status: u16, url: String },

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("not found")]
    NotFound,

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Convenience constructor for an unclassified error.
    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }

    /// Convenience constructor for a storage backend failure.
    pub fn storage(msg: impl Into<String>) -> Self {
        Error::Storage(msg.into())
    }
}
