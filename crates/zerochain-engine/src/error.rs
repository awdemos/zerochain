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

    #[error("failed to load workflows: {0}")]
    WorkflowLoadPartial(String),

    #[error("LLM error: {0}")]
    Llm(#[from] zerochain_llm::error::LLMError),



    #[error("profile validation failed: {0}")]
    ProfileValidation(zerochain_llm::error::LLMError),

    #[error("missing environment variable: {0}")]
    MissingEnv(String),

    #[error("CoW snapshot error for stage {stage}: {source}")]
    CowSnapshot { stage: String, #[source] source: zerochain_fs::error::FsError },

    #[error("CoW restore error for stage {stage}: {source}")]
    CowRestore { stage: String, #[source] source: zerochain_fs::error::FsError },

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
}

impl DaemonError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
