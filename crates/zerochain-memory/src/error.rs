use std::path::PathBuf;
use thiserror::Error;
use zerochain_error::ZerochainError;

/// Errors produced by the vector memory subsystem.
#[derive(Error, Debug)]
pub enum MemoryError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("embedding model error: {0}")]
    Embedding(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("memory error: {0}")]
    Other(String),
}

impl From<MemoryError> for ZerochainError {
    fn from(err: MemoryError) -> Self {
        match err {
            MemoryError::Io { path, source } => ZerochainError::Io { path, source },
            MemoryError::Embedding(msg) => ZerochainError::Llm {
                message: format!("embedding: {msg}"),
            },
            MemoryError::InvalidInput(msg) => ZerochainError::InvalidInput { message: msg },
            MemoryError::Serialization(msg) => ZerochainError::Serialization { message: msg },
            MemoryError::Other(msg) => ZerochainError::Other { message: msg },
        }
    }
}

impl From<ZerochainError> for MemoryError {
    fn from(err: ZerochainError) -> Self {
        match err {
            ZerochainError::Io { path, source } => MemoryError::Io { path, source },
            ZerochainError::InvalidInput { message } => MemoryError::InvalidInput(message),
            ZerochainError::Serialization { message } => MemoryError::Serialization(message),
            ZerochainError::Llm { message } => MemoryError::Embedding(message),
            other => MemoryError::Other(format!("{other}")),
        }
    }
}

/// Create a `MemoryError` carrying the path that caused the I/O failure.
pub fn io_err(path: impl AsRef<std::path::Path>, source: std::io::Error) -> MemoryError {
    MemoryError::Io {
        path: path.as_ref().to_path_buf(),
        source,
    }
}

impl From<serde_json::Error> for MemoryError {
    fn from(source: serde_json::Error) -> Self {
        MemoryError::Serialization(source.to_string())
    }
}
