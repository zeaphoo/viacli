//! Error types shared across the core engine.

use std::path::PathBuf;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("io error: {0}")]
    PlainIo(#[from] std::io::Error),

    #[error("failed to parse {what}: {reason}")]
    Parse { what: String, reason: String },

    #[error("discovery source `{backend}` is unavailable: {reason}")]
    BackendUnavailable { backend: String, reason: String },

    #[error("discovery source `{backend}` failed: {reason}")]
    BackendFailed { backend: String, reason: String },

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// A capability this build was not compiled with. Used for the optional
    /// features, where "unavailable backend" would be the wrong word.
    #[error("{0}")]
    Unsupported(String),
}

impl Error {
    /// The reason inside a backend failure, without the `discovery source ...`
    /// wrapper, so callers can re-package it without repeating the prefix.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Error::BackendUnavailable { reason, .. } | Error::BackendFailed { reason, .. } => {
                Some(reason)
            }
            _ => None,
        }
    }

    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }

    pub fn parse(what: impl Into<String>, reason: impl Into<String>) -> Self {
        Error::Parse {
            what: what.into(),
            reason: reason.into(),
        }
    }

    pub fn unavailable(backend: impl Into<String>, reason: impl Into<String>) -> Self {
        Error::BackendUnavailable {
            backend: backend.into(),
            reason: reason.into(),
        }
    }

    pub fn unsupported(reason: impl Into<String>) -> Self {
        Error::Unsupported(reason.into())
    }

    pub fn failed(backend: impl Into<String>, reason: impl Into<String>) -> Self {
        Error::BackendFailed {
            backend: backend.into(),
            reason: reason.into(),
        }
    }

    /// True when the failure is a missing privilege (raw socket / CAP_NET_RAW).
    pub fn is_permission_denied(&self) -> bool {
        let text = self.to_string().to_ascii_lowercase();
        text.contains("permission denied")
            || text.contains("operation not permitted")
            || text.contains("eperm")
    }
}
