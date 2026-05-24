use std::path::PathBuf;
use zerochain_error::ZerochainError;

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

    #[error("invalid workflow name: {name}")]
    InvalidWorkflowName { name: String },

    #[error("YAML parse error in {path}: {source}")]
    YamlParse {
        path: PathBuf,
        #[source]
        source: serde_yml::Error,
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

    #[error("shared store load error at {path}: {reason}")]
    SharedStoreLoad { path: PathBuf, reason: String },
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn io_err(path: impl Into<PathBuf>, source: std::io::Error) -> Error {
    Error::Io {
        path: path.into(),
        source,
    }
}

impl From<Error> for ZerochainError {
    fn from(err: Error) -> Self {
        match err {
            Error::Io { path, source } => ZerochainError::Io { path, source },
            Error::InvalidStageName { name } => {
                ZerochainError::Stage {
                    message: format!("invalid stage name: {name}"),
                }
            }
            Error::InvalidWorkflowName { name } => {
                ZerochainError::Workflow {
                    message: format!("invalid workflow name: {name}"),
                }
            }
            Error::YamlParse { path, source } => ZerochainError::YamlParse {
                message: format!("{path:?}: {source}"),
            },
            Error::MissingContext { stage } => ZerochainError::Stage {
                message: format!("missing CONTEXT.md in stage {stage}"),
            },
            Error::MissingField { field, context } => ZerochainError::InvalidInput {
                message: format!("missing field '{field}' in {context}"),
            },
            Error::WorkflowNotFound { path } => ZerochainError::NotFound {
                message: format!("workflow directory not found: {path:?}"),
            },
            Error::NoStages { path } => ZerochainError::Workflow {
                message: format!("no stages found in workflow at {path:?}"),
            },
            Error::PlanError { reason } => ZerochainError::Workflow {
                message: format!("execution plan error: {reason}"),
            },
            Error::JjError { message } => ZerochainError::Other {
                message: format!("jj error: {message}"),
            },
            Error::JjNotInstalled => ZerochainError::Other {
                message: "jj is not installed or not found in PATH".to_string(),
            },
            Error::TaskParse { path, reason } => ZerochainError::InvalidInput {
                message: format!("task parse error in {path:?}: {reason}"),
            },
            Error::Lua { message } => ZerochainError::Lua { message },
            Error::SharedStoreLoad { path, reason } => ZerochainError::Other {
                message: format!("shared store load error at {path:?}: {reason}"),
            },
        }
    }
}

impl From<ZerochainError> for Error {
    fn from(err: ZerochainError) -> Self {
        match err {
            ZerochainError::Io { path, source } => Error::Io { path, source },
            ZerochainError::NotFound { message } => Error::WorkflowNotFound {
                path: PathBuf::from(message),
            },
            ZerochainError::InvalidInput { message } => Error::MissingField {
                field: message,
                context: "unknown".to_string(),
            },
            ZerochainError::Configuration { message } => Error::PlanError { reason: message },
            ZerochainError::Workflow { message } => Error::PlanError { reason: message },
            ZerochainError::Stage { message } => Error::InvalidStageName { name: message },
            ZerochainError::YamlParse { message } => Error::TaskParse {
                path: PathBuf::new(),
                reason: message,
            },
            ZerochainError::Lua { message } => Error::Lua { message },
            other => Error::PlanError {
                reason: other.to_string(),
            },
        }
    }
}
