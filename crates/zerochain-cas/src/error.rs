use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur when interacting with the content-addressed store.
#[derive(Error, Debug)]
pub enum CasError {
    #[error("content not found: {0}")]
    NotFound(String),

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid CID: {0}")]
    InvalidCid(String),

    #[error("S3 error: {0}")]
    S3(String),

    #[error("configuration error: {0}")]
    Configuration(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("store directory error: {path}")]
    StoreDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("unsupported operation: {0}")]
    Unsupported(String),
}

impl CasError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl CasError {
    /// Returns true if this error indicates the requested content was not found.
    pub fn is_not_found(&self) -> bool {
        matches!(self, CasError::NotFound(_))
    }
}

pub type Result<T> = std::result::Result<T, CasError>;
