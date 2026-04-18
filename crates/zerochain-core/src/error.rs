use std::path::PathBuf;

/// Errors produced by zerochain-core operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid stage directory name: {name}")]
    InvalidStageName { name: String },

    #[error("YAML parse error in {path}: {source}")]
    YamlParse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("missing CONTEXT.md in stage {stage}")]
    MissingContext { stage: String },

    #[error("missing required field '{field}' in {context}")]
    MissingField { field: String, context: String },

    #[error("workflow directory not found: {path}")]
    WorkflowNotFound { path: PathBuf },

    #[error("no stages found in workflow at {path}")]
    NoStages { path: PathBuf },

    #[error("execution plan error: {reason}")]
    PlanError { reason: String },

    #[error("jj (Jujutsu) error: {message}")]
    JjError { message: String },

    #[error("jj is not installed or not found in PATH")]
    JjNotInstalled,

    #[error("task parse error in {path}: {reason}")]
    TaskParse { path: PathBuf, reason: String },

    #[error("Lua error: {message}")]
    Lua { message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

#[allow(dead_code)]
pub(crate) fn io_err(path: impl Into<PathBuf>, source: std::io::Error) -> Error {
    Error::Io {
        path: path.into(),
        source,
    }
}
