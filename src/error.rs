use std::io;

/// Crate-wide result type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO 错误：{0}")]
    Io(#[from] io::Error),

    #[error("HTTP 错误：{0}")]
    Http(#[from] reqwest::Error),

    #[error("URL 解析错误：{0}")]
    Url(#[from] url::ParseError),

    #[error("JSON 错误：{0}")]
    Json(#[from] serde_json::Error),

    #[error("配置错误：{0}")]
    Config(String),

    #[error("存储错误：{0}")]
    Storage(String),

    #[error("socket 错误：{0}")]
    Socket(String),

    #[error("websocket 错误：{0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("主控返回了错误：{0}")]
    Service(String),

    #[error("{url} 返回了意外的状态 {status}")]
    Status { status: u16, url: String },

    #[error("上游不可用，{retry_in_ms} 毫秒后重试")]
    UpstreamUnavailable { retry_in_ms: u64 },

    #[error("超时：{0}")]
    Timeout(String),

    #[error("未找到")]
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

    /// Whether restarting the process could plausibly clear this.
    ///
    /// A configuration error is the same error on the next attempt, so the
    /// supervisor must stop rather than loop behind a backoff.
    pub fn is_fatal(&self) -> bool {
        matches!(self, Error::Config(_))
    }
}

#[cfg(test)]
#[path = "error_test.rs"]
mod tests;
