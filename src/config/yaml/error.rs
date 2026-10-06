//! The reader's error type.

/// A YAML reader failure, always carrying the 1-based input line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("第 {line} 行：{message}")]
pub(crate) struct YamlError {
    /// 1-based line the failure was detected on.
    pub(crate) line: usize,
    /// What went wrong.
    pub(crate) message: String,
}

impl YamlError {
    /// Build an error anchored at line.
    pub(crate) fn new(line: usize, message: impl Into<String>) -> Self {
        YamlError {
            line,
            message: message.into(),
        }
    }
}
