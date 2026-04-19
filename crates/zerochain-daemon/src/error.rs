use std::path::PathBuf;

/// Errors produced by the zerochain daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("workflow not found: {0}")]
    WorkflowNotFound(String),

    #[error("stage not found: {0}")]
    StageNotFound(String),

    #[error("invalid stage id: {0}")]
    InvalidStageId(String),

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("workflow error: {0}")]
    Workflow(#[from] zerochain_core::error::Error),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("Lua error: {0}")]
    Lua(String),

    #[error("profile validation failed: {0}")]
    ProfileValidation(String),

    #[error("missing environment variable: {0}")]
    MissingEnv(String),
}

impl DaemonError {
    /// Create an I/O error with the given path.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<std::io::Error> for DaemonError {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            path: PathBuf::from("<unknown>"),
            source,
        }
    }
}
