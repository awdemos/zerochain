use std::path::PathBuf;

/// Errors produced by zerochain-fs operations.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("snapshot failed: source {src_path} -> target {target}: {reason}")]
    SnapshotFailed {
        src_path: PathBuf,
        target: PathBuf,
        reason: String,
    },

    #[error("atomic write failed for {path}: {reason}")]
    AtomicWriteFailed { path: PathBuf, reason: String },

    #[error("marker operation failed in {dir}: {reason}")]
    MarkerFailed { dir: PathBuf, reason: String },

    #[error("lock already held at {path} by process {pid}")]
    LockHeld { path: PathBuf, pid: u32 },

    #[error("btrfs command '{command}' failed: {reason}")]
    BtrfsCommandFailed { command: String, reason: String },

    #[error("btrfs subvolume error at {path}: {reason}")]
    SubvolumeError { path: PathBuf, reason: String },
}

pub type Result<T> = std::result::Result<T, FsError>;

/// Helper to wrap an io::Error with path context.
pub(crate) fn io_err(path: impl Into<PathBuf>, source: std::io::Error) -> FsError {
    FsError::Io {
        path: path.into(),
        source,
    }
}
