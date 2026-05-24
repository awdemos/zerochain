use std::path::PathBuf;
use thiserror::Error;

/// Unified error type for the zerochain workspace.
///
/// Provides a shared vocabulary for cross-crate error propagation
/// while preserving each crate's typed error API.
#[derive(Error, Debug)]
pub enum ZerochainError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("not found: {message}")]
    NotFound { message: String },

    #[error("invalid input: {message}")]
    InvalidInput { message: String },

    #[error("configuration error: {message}")]
    Configuration { message: String },

    #[error("unsupported operation: {message}")]
    Unsupported { message: String },

    #[error("rate limited: retry after {retry_after_ms:?}ms")]
    RateLimited { retry_after_ms: Option<u64> },

    #[error("authentication failed: {message}")]
    Auth { message: String },

    #[error("workflow error: {message}")]
    Workflow { message: String },

    #[error("stage error: {message}")]
    Stage { message: String },

    #[error("broker error: {message}")]
    Broker { message: String },

    #[error("LLM error: {message}")]
    Llm { message: String },

    #[error("CAS error: {message}")]
    Cas { message: String },

    #[error("filesystem error: {message}")]
    Fs { message: String },

    #[error("serialization error: {message}")]
    Serialization { message: String },

    #[error("YAML parse error: {message}")]
    YamlParse { message: String },

    #[error("container error: {message}")]
    Container { message: String },

    #[error("missing environment variable: {var}")]
    MissingEnv { var: String },

    #[error("Lua error: {message}")]
    Lua { message: String },

    #[error("{message}")]
    Other { message: String },
}

pub type Result<T> = std::result::Result<T, ZerochainError>;
