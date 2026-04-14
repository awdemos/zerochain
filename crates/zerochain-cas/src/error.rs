use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur when interacting with the content-addressed store.
#[derive(Error, Debug)]
pub enum CasError {
    /// The requested content identifier was not found in the store.
    #[error("content not found: {0}")]
    NotFound(String),

    /// An I/O error occurred during a filesystem operation.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A CID string failed to parse.
    #[error("invalid CID: {0}")]
    InvalidCid(String),

    /// A serialization or deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// The base directory could not be created or is inaccessible.
    #[error("store directory error: {path}")]
    StoreDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, CasError>;
