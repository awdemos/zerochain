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
        }
    }
}

impl From<std::io::Error> for MemoryError {
    fn from(source: std::io::Error) -> Self {
        MemoryError::Io {
            path: PathBuf::new(),
            source,
        }
    }
}

impl From<serde_json::Error> for MemoryError {
    fn from(source: serde_json::Error) -> Self {
        MemoryError::Serialization(source.to_string())
    }
}
