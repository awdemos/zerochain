use std::path::PathBuf;

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

    #[error("LLM error: {0}")]
    Llm(#[from] zerochain_llm::error::LLMError),

    #[error("Lua error: {0}")]
    Lua(String),

    #[error("profile validation failed: {0}")]
    ProfileValidation(zerochain_llm::error::LLMError),

    #[error("missing environment variable: {0}")]
    MissingEnv(String),

    #[error("CoW snapshot error: {0}")]
    CowSnapshot(String),

    #[error("CoW restore error: {0}")]
    CowRestore(String),

    #[error("container spawn error: {0}")]
    ContainerSpawn(String),

    #[error("container execution error: {0}")]
    ContainerExec(String),

    #[error("filesystem error: {0}")]
    Fs(#[from] zerochain_fs::error::FsError),
}

impl DaemonError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
