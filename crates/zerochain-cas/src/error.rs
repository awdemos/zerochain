use std::path::PathBuf;
use thiserror::Error;
use zerochain_error::ZerochainError;

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
    #[must_use] pub fn is_not_found(&self) -> bool {
        matches!(self, CasError::NotFound(_))
    }
}

pub type Result<T> = std::result::Result<T, CasError>;

impl From<CasError> for ZerochainError {
    fn from(err: CasError) -> Self {
        match err {
            CasError::NotFound(msg) => ZerochainError::NotFound { message: msg },
            CasError::Io { path, source } => ZerochainError::Io { path, source },
            CasError::InvalidCid(msg) => ZerochainError::InvalidInput { message: msg },
            CasError::S3(msg) => ZerochainError::Cas { message: msg },
            CasError::Configuration(msg) => ZerochainError::Configuration { message: msg },
            CasError::Serialization(e) => {
                ZerochainError::Serialization { message: e.to_string() }
            }
            CasError::StoreDirectory { path, source } => ZerochainError::Io { path, source },
            CasError::Unsupported(msg) => ZerochainError::Unsupported { message: msg },
        }
    }
}

impl From<ZerochainError> for CasError {
    fn from(err: ZerochainError) -> Self {
        match err {
            ZerochainError::Io { path, source } => CasError::Io { path, source },
            ZerochainError::NotFound { message } => CasError::NotFound(message),
            ZerochainError::InvalidInput { message } => CasError::InvalidCid(message),
            ZerochainError::Configuration { message } => CasError::Configuration(message),
            ZerochainError::Unsupported { message } => CasError::Unsupported(message),
            ZerochainError::Serialization { message } => {
                CasError::Configuration(format!("serialization: {message}"))
            }
            ZerochainError::Cas { message } => CasError::S3(message),
            other => CasError::Configuration(other.to_string()),
        }
    }
}
