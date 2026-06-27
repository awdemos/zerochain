use std::path::PathBuf;
use zerochain_error::ZerochainError;

/// Errors produced by the zerochain daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("workflow not found: {0}")]
    WorkflowNotFound(String),

    #[error("stage not found: {0}")]
    StageNotFound(String),

    #[error("invalid stage id: {stage_id}")]
    InvalidStageId {
        stage_id: String,
        #[source]
        source: zerochain_core::error::Error,
    },

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("workflow error: {0}")]
    Workflow(#[from] zerochain_core::error::Error),

    #[error("failed to load workflows: {0}")]
    WorkflowLoadPartial(String),

    #[error("LLM error: {0}")]
    Llm(#[from] zerochain_llm::error::LLMError),

    #[error("profile validation failed: {0}")]
    ProfileValidation(zerochain_llm::error::LLMError),

    #[error("missing environment variable: {0}")]
    MissingEnv(String),

    #[error("CoW snapshot error for stage {stage}: {source}")]
    CowSnapshot {
        stage: String,
        #[source]
        source: zerochain_fs::error::FsError,
    },

    #[error("CoW restore error for stage {stage}: {source}")]
    CowRestore {
        stage: String,
        #[source]
        source: zerochain_fs::error::FsError,
    },

    #[error("container spawn error: {0}")]
    ContainerSpawn(#[source] std::io::Error),

    #[error("container execution error: {0}")]
    ContainerExec(String),

    #[error("container image operation failed for {image}: {stderr}")]
    ContainerImage { image: String, stderr: String },

    #[error("no container runtime found (need docker or podman)")]
    ContainerRuntimeNotFound,

    #[error("filesystem error: {0}")]
    Fs(#[from] zerochain_fs::error::FsError),

    #[error("CAS error: {0}")]
    Cas(#[from] zerochain_cas::CasError),
}

impl DaemonError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<DaemonError> for ZerochainError {
    fn from(err: DaemonError) -> Self {
        match err {
            DaemonError::WorkflowNotFound(msg) => ZerochainError::NotFound {
                message: format!("workflow not found: {msg}"),
            },
            DaemonError::StageNotFound(msg) => ZerochainError::NotFound {
                message: format!("stage not found: {msg}"),
            },
            DaemonError::InvalidStageId { stage_id, source } => ZerochainError::Stage {
                message: format!("invalid stage id {stage_id}: {source}"),
            },
            DaemonError::Io { path, source } => ZerochainError::Io { path, source },
            DaemonError::Workflow(e) => ZerochainError::from(e),
            DaemonError::WorkflowLoadPartial(msg) => ZerochainError::Workflow { message: msg },
            DaemonError::Llm(e) => ZerochainError::from(e),
            DaemonError::ProfileValidation(e) => ZerochainError::Llm {
                message: format!("profile validation: {e}"),
            },
            DaemonError::MissingEnv(var) => ZerochainError::MissingEnv { var },
            DaemonError::CowSnapshot { stage, source } => ZerochainError::Fs {
                message: format!("CoW snapshot for stage {stage}: {source}"),
            },
            DaemonError::CowRestore { stage, source } => ZerochainError::Fs {
                message: format!("CoW restore for stage {stage}: {source}"),
            },
            DaemonError::ContainerSpawn(source) => ZerochainError::Container {
                message: format!("container spawn: {source}"),
            },
            DaemonError::ContainerExec(msg) => ZerochainError::Container { message: msg },
            DaemonError::ContainerImage { image, stderr } => ZerochainError::Container {
                message: format!("container image {image}: {stderr}"),
            },
            DaemonError::ContainerRuntimeNotFound => ZerochainError::Container {
                message: "no container runtime found (need docker or podman)".to_string(),
            },
            DaemonError::Fs(e) => ZerochainError::from(e),
            DaemonError::Cas(e) => ZerochainError::from(e),
        }
    }
}

impl From<ZerochainError> for DaemonError {
    fn from(err: ZerochainError) -> Self {
        match err {
            ZerochainError::NotFound { message } => DaemonError::WorkflowNotFound(message),
            ZerochainError::Io { path, source } => DaemonError::Io { path, source },
            ZerochainError::Workflow { message } => DaemonError::WorkflowLoadPartial(message),
            ZerochainError::Stage { message } => DaemonError::StageNotFound(message),
            ZerochainError::Llm { message } => {
                DaemonError::Llm(zerochain_llm::error::LLMError::Other(message))
            }
            ZerochainError::Fs { message } => {
                DaemonError::Fs(zerochain_fs::error::FsError::SubvolumeError {
                    path: PathBuf::new(),
                    reason: message,
                })
            }
            ZerochainError::Container { message } => DaemonError::ContainerExec(message),
            ZerochainError::MissingEnv { var } => DaemonError::MissingEnv(var),
            other => DaemonError::WorkflowLoadPartial(other.to_string()),
        }
    }
}
