use std::path::Path;
use std::sync::Arc;

use crate::error::{io_err, FsError, Result};

#[async_trait::async_trait]
pub trait CowPlatform: Send + Sync {
    async fn snapshot(&self, source_dir: &Path, target_dir: &Path) -> Result<()>;

    async fn is_available(&self) -> bool;

    fn name(&self) -> &str;

    /// Ensure the workflow root directory exists, possibly as a subvolume.
    async fn prepare_workflow_root(&self, path: &Path) -> Result<()> {
        tokio::fs::create_dir_all(path)
            .await
            .map_err(|e| io_err(path, e))
    }

    /// Ensure a stage directory exists, possibly as a subvolume.
    async fn prepare_stage_dir(&self, path: &Path) -> Result<()> {
        tokio::fs::create_dir_all(path)
            .await
            .map_err(|e| io_err(path, e))
    }

    /// Remove a stage directory, using subvolume deletion if applicable.
    async fn remove_stage_dir(&self, path: &Path) -> Result<()> {
        tokio::fs::remove_dir_all(path)
            .await
            .map_err(|e| io_err(path, e))
    }

    /// Remove a snapshot directory, using subvolume deletion if applicable.
    async fn remove_snapshot(&self, path: &Path) -> Result<()> {
        tokio::fs::remove_dir_all(path)
            .await
            .map_err(|e| io_err(path, e))
    }
}

pub struct DirectoryCow;

/// Controls how zerochain uses Btrfs subvolumes for isolation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SubvolumeMode {
    /// No subvolumes are created; plain directories are used.
    #[default]
    Off,
    /// The workflow root is created as a subvolume.
    Workflow,
    /// The workflow root and each stage are created as subvolumes.
    Stage,
}

impl SubvolumeMode {
    /// Parse from the `ZEROCHAIN_BTRFS_SUBVOLUME_MODE` environment variable.
    #[must_use]
    pub fn from_env() -> Self {
        std::env::var("ZEROCHAIN_BTRFS_SUBVOLUME_MODE")
            .unwrap_or_default()
            .parse()
            .unwrap_or_default()
    }

    /// Load the persisted mode from `{workflow_root}/.subvolume-mode`, if any.
    pub async fn load(workflow_root: &Path) -> Option<Self> {
        let marker = workflow_root.join(".subvolume-mode");
        let content = tokio::fs::read_to_string(&marker).await.ok()?;
        content.trim().parse().ok()
    }

    /// Persist this mode to `{workflow_root}/.subvolume-mode`.
    pub async fn save(self, workflow_root: &Path) -> Result<()> {
        let marker = workflow_root.join(".subvolume-mode");
        tokio::fs::write(&marker, self.to_string())
            .await
            .map_err(|e| io_err(marker, e))
    }
}

impl std::fmt::Display for SubvolumeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => write!(f, "off"),
            Self::Workflow => write!(f, "workflow"),
            Self::Stage => write!(f, "stage"),
        }
    }
}

impl std::str::FromStr for SubvolumeMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "workflow" => Ok(Self::Workflow),
            "stage" => Ok(Self::Stage),
            other => Err(format!("unknown subvolume mode: {other}")),
        }
    }
}

pub struct BtrfsCow {
    mode: SubvolumeMode,
}

/// A no-op `CoW` backend that silently succeeds.
pub struct NoopCow;

#[async_trait::async_trait]
impl CowPlatform for NoopCow {
    async fn snapshot(&self, _source_dir: &Path, _target_dir: &Path) -> Result<()> {
        Ok(())
    }

    async fn is_available(&self) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "disabled"
    }
}

pub async fn detect_backend(workspace_path: &Path) -> Arc<dyn CowPlatform + Send + Sync> {
    let mode = SubvolumeMode::from_env();
    let btrfs = BtrfsCow::new(mode);
    if btrfs.is_available().await && BtrfsCow::is_btrfs_filesystem(workspace_path).await {
        match mode {
            SubvolumeMode::Off => tracing::info!("CoW backend: btrfs (zero-copy snapshots)"),
            SubvolumeMode::Workflow => {
                tracing::info!("CoW backend: btrfs with workflow subvolumes")
            }
            SubvolumeMode::Stage => {
                tracing::info!("CoW backend: btrfs with per-stage subvolumes")
            }
        }
        Arc::new(btrfs)
    } else {
        tracing::info!("CoW backend: directory (file-level copy)");
        Arc::new(DirectoryCow)
    }
}

impl BtrfsCow {
    /// Create a Btrfs-backed CoW backend with the given subvolume mode.
    #[must_use]
    pub fn new(mode: SubvolumeMode) -> Self {
        Self { mode }
    }

    pub async fn create_subvolume(&self, path: &Path) -> Result<()> {
        let output = tokio::process::Command::new("btrfs")
            .args(["subvolume", "create", &path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| FsError::BtrfsCommandFailed {
                command: "subvolume create".into(),
                reason: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(FsError::SubvolumeError {
                path: path.to_path_buf(),
                reason: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }
        Ok(())
    }

    pub async fn delete_subvolume(&self, path: &Path) -> Result<()> {
        let output = tokio::process::Command::new("btrfs")
            .args(["subvolume", "delete", &path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| FsError::BtrfsCommandFailed {
                command: "subvolume delete".into(),
                reason: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(FsError::SubvolumeError {
                path: path.to_path_buf(),
                reason: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }
        Ok(())
    }

    /// Returns `true` if `path` is a Btrfs subvolume.
    pub async fn is_subvolume(&self, path: &Path) -> bool {
        match tokio::process::Command::new("btrfs")
            .args(["subvolume", "show", &path.to_string_lossy()])
            .output()
            .await
        {
            Ok(output) => output.status.success(),
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "failed to probe subvolume status");
                false
            }
        }
    }

    /// Delete a subvolume, falling back to plain directory removal if the path
    /// is not actually a subvolume.
    pub async fn delete_subvolume_or_dir(&self, path: &Path) -> Result<()> {
        if !self.is_subvolume(path).await {
            return tokio::fs::remove_dir_all(path)
                .await
                .map_err(|e| io_err(path, e));
        }
        match self.delete_subvolume(path).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let reason = e.to_string().to_lowercase();
                if reason.contains("not a subvolume") {
                    tracing::warn!(
                        path = %path.display(),
                        "btrfs subvolume delete reported not a subvolume, falling back to directory removal"
                    );
                    tokio::fs::remove_dir_all(path)
                        .await
                        .map_err(|e| io_err(path, e))
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Recursively delete a subvolume and any nested subvolumes inside it.
    pub async fn delete_subvolume_recursive(&self, path: &Path) -> Result<()> {
        let output = tokio::process::Command::new("btrfs")
            .args(["subvolume", "delete", "-r", &path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| FsError::BtrfsCommandFailed {
                command: "subvolume delete -r".into(),
                reason: e.to_string(),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.to_lowercase().contains("not a subvolume") {
                return tokio::fs::remove_dir_all(path)
                    .await
                    .map_err(|e| io_err(path, e));
            }
            return Err(FsError::SubvolumeError {
                path: path.to_path_buf(),
                reason: stderr.to_string(),
            });
        }
        Ok(())
    }

    /// Determine the effective subvolume mode for operations under `path`.
    ///
    /// Walks up the directory tree looking for `{dir}/.subvolume-mode`. If
    /// found, the persisted mode is used; otherwise the environment variable
    /// mode is used.
    async fn effective_mode(&self, path: &Path) -> SubvolumeMode {
        let mut current = Some(path);
        while let Some(dir) = current {
            if let Some(mode) = SubvolumeMode::load(dir).await {
                return mode;
            }
            current = dir.parent();
        }
        self.mode
    }

    pub async fn list_subvolumes(&self, path: &Path) -> Result<Vec<String>> {
        let output = tokio::process::Command::new("btrfs")
            .args(["subvolume", "list", &path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| FsError::BtrfsCommandFailed {
                command: "subvolume list".into(),
                reason: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(FsError::SubvolumeError {
                path: path.to_path_buf(),
                reason: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let mut names = Vec::new();
        for line in text.lines() {
            if let Some(name) = line.rsplit(' ').next() {
                names.push(name.to_string());
            }
        }
        Ok(names)
    }

    pub async fn send_snapshot(
        &self,
        parent: &Path,
        child: &Path,
        output_path: &Path,
    ) -> Result<()> {
        let parent_str = parent.to_string_lossy().to_string();
        let child_str = child.to_string_lossy().to_string();

        let mut child_proc = tokio::process::Command::new("btrfs")
            .args(["send", "-p", &parent_str, &child_str])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| FsError::BtrfsCommandFailed {
                command: "send".into(),
                reason: e.to_string(),
            })?;

        let stdout = child_proc
            .stdout
            .take()
            .ok_or_else(|| FsError::BtrfsCommandFailed {
                command: "send".into(),
                reason: "no stdout pipe".into(),
            })?;

        let mut reader = tokio::io::BufReader::new(stdout);
        let mut file = tokio::fs::File::create(output_path)
            .await
            .map_err(|e| FsError::Io {
                path: output_path.to_path_buf(),
                source: e,
            })?;

        tokio::io::copy(&mut reader, &mut file)
            .await
            .map_err(|e| FsError::BtrfsCommandFailed {
                command: "send".into(),
                reason: e.to_string(),
            })?;

        let status = child_proc
            .wait()
            .await
            .map_err(|e| FsError::BtrfsCommandFailed {
                command: "send".into(),
                reason: e.to_string(),
            })?;

        if !status.success() {
            return Err(FsError::BtrfsCommandFailed {
                command: "send".into(),
                reason: format!("exit code: {}", status.code().unwrap_or(-1)),
            });
        }
        Ok(())
    }

    pub async fn is_btrfs_filesystem(path: &Path) -> bool {
        #[cfg(target_os = "linux")]
        {
            tokio::process::Command::new("stat")
                .args(["-f", "--format", "%T", &path.to_string_lossy()])
                .output()
                .await
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "btrfs")
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            false
        }
    }
}

#[async_trait::async_trait]
impl CowPlatform for BtrfsCow {
    async fn snapshot(&self, source_dir: &Path, target_dir: &Path) -> Result<()> {
        if !tokio::fs::try_exists(source_dir)
            .await
            .map_err(|e| io_err(source_dir, e))?
        {
            return Err(FsError::SnapshotFailed {
                src_path: source_dir.to_path_buf(),
                target: target_dir.to_path_buf(),
                reason: "source subvolume does not exist".into(),
            });
        }

        if tokio::fs::try_exists(target_dir)
            .await
            .map_err(|e| io_err(target_dir, e))?
        {
            return Err(FsError::SnapshotFailed {
                src_path: source_dir.to_path_buf(),
                target: target_dir.to_path_buf(),
                reason: "target already exists".into(),
            });
        }

        let output = tokio::process::Command::new("btrfs")
            .args([
                "subvolume",
                "snapshot",
                &source_dir.to_string_lossy(),
                &target_dir.to_string_lossy(),
            ])
            .output()
            .await
            .map_err(|e| FsError::BtrfsCommandFailed {
                command: "subvolume snapshot".into(),
                reason: e.to_string(),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let lower = stderr.to_lowercase();
            if lower.contains("not a subvolume") || lower.contains("not a btrfs") {
                tracing::warn!(
                    source = %source_dir.display(),
                    target = %target_dir.display(),
                    "btrfs snapshot failed: source is not a subvolume, falling back to directory copy"
                );
                return DirectoryCow.snapshot(source_dir, target_dir).await;
            }
            return Err(FsError::SnapshotFailed {
                src_path: source_dir.to_path_buf(),
                target: target_dir.to_path_buf(),
                reason: stderr.to_string(),
            });
        }

        tracing::debug!(
            source = %source_dir.display(),
            target = %target_dir.display(),
            "btrfs snapshot created"
        );

        Ok(())
    }

    async fn is_available(&self) -> bool {
        tokio::process::Command::new("btrfs")
            .arg("--version")
            .output()
            .await
            .is_ok()
    }

    async fn prepare_workflow_root(&self, path: &Path) -> Result<()> {
        let mode = self.mode;
        let created = match mode {
            SubvolumeMode::Off => {
                tokio::fs::create_dir_all(path)
                    .await
                    .map_err(|e| io_err(path, e))?;
                false
            }
            SubvolumeMode::Workflow | SubvolumeMode::Stage => {
                if tokio::fs::try_exists(path)
                    .await
                    .map_err(|e| io_err(path, e))?
                {
                    false
                } else if let Err(e) = self.create_subvolume(path).await {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to create workflow root subvolume, falling back to directory"
                    );
                    tokio::fs::create_dir_all(path)
                        .await
                        .map_err(|e| io_err(path, e))?;
                    false
                } else {
                    true
                }
            }
        };

        // Persist the mode whenever we create the workflow root. If the root
        // already existed, preserve any existing marker.
        if created {
            if let Err(e) = mode.save(path).await {
                tracing::warn!(path = %path.display(), error = %e, "failed to save subvolume mode marker");
            }
        }
        Ok(())
    }

    async fn prepare_stage_dir(&self, path: &Path) -> Result<()> {
        let mode = self.effective_mode(path).await;
        match mode {
            SubvolumeMode::Off | SubvolumeMode::Workflow => {
                tokio::fs::create_dir_all(path)
                    .await
                    .map_err(|e| io_err(path, e))
            }
            SubvolumeMode::Stage => {
                if tokio::fs::try_exists(path)
                    .await
                    .map_err(|e| io_err(path, e))?
                {
                    return Ok(());
                }
                if let Err(e) = self.create_subvolume(path).await {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to create stage subvolume, falling back to directory"
                    );
                    tokio::fs::create_dir_all(path)
                        .await
                        .map_err(|e| io_err(path, e))
                } else {
                    Ok(())
                }
            }
        }
    }

    async fn remove_stage_dir(&self, path: &Path) -> Result<()> {
        if self.is_subvolume(path).await {
            self.delete_subvolume_recursive(path).await
        } else {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|e| io_err(path, e))
        }
    }

    async fn remove_snapshot(&self, path: &Path) -> Result<()> {
        if self.is_subvolume(path).await {
            self.delete_subvolume_recursive(path).await
        } else {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|e| io_err(path, e))
        }
    }

    fn name(&self) -> &'static str {
        "btrfs"
    }
}

#[async_trait::async_trait]
impl CowPlatform for DirectoryCow {
    async fn snapshot(&self, source_dir: &Path, target_dir: &Path) -> Result<()> {
        if !tokio::fs::try_exists(source_dir)
            .await
            .map_err(|e| io_err(source_dir, e))?
        {
            return Err(FsError::SnapshotFailed {
                src_path: source_dir.to_path_buf(),
                target: target_dir.to_path_buf(),
                reason: "source directory does not exist".into(),
            });
        }

        if tokio::fs::try_exists(target_dir)
            .await
            .map_err(|e| io_err(target_dir, e))?
        {
            return Err(FsError::SnapshotFailed {
                src_path: source_dir.to_path_buf(),
                target: target_dir.to_path_buf(),
                reason: "target directory already exists".into(),
            });
        }

        // Fast path: ask the system cp to use reflink clones when available.
        // On Btrfs/XFS/APFS this produces a near-zero-copy snapshot; on other
        // filesystems cp falls back to a normal copy. We only attempt this on
        // platforms where cp supports --reflink=auto.
        #[cfg(unix)]
        if let Ok(output) = tokio::process::Command::new("cp")
            .args([
                "-a",
                "--reflink=auto",
                &source_dir.to_string_lossy(),
                &target_dir.to_string_lossy(),
            ])
            .output()
            .await
        {
            if output.status.success() {
                tracing::debug!(
                    source = %source_dir.display(),
                    target = %target_dir.display(),
                    "directory snapshot created via cp --reflink=auto"
                );
                return Ok(());
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::debug!(
                source = %source_dir.display(),
                target = %target_dir.display(),
                stderr = %stderr,
                "cp --reflink=auto failed; falling back to recursive copy"
            );
        }

        copy_dir_recursive(source_dir, target_dir).await?;

        tracing::debug!(
            source = %source_dir.display(),
            target = %target_dir.display(),
            "directory snapshot created via recursive copy"
        );

        Ok(())
    }

    async fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "directory"
    }
}

fn copy_dir_recursive<'a>(
    source: &'a Path,
    target: &'a Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        tokio::fs::create_dir_all(target)
            .await
            .map_err(|e| io_err(target, e))?;

        let mut entries = tokio::fs::read_dir(source)
            .await
            .map_err(|e| io_err(source, e))?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| io_err(source, e))? {
            let src_path = entry.path();
            let file_name = entry.file_name();
            let dst_path = target.join(&file_name);

            let file_type = entry.file_type().await.map_err(|e| io_err(&src_path, e))?;

            if file_type.is_dir() {
                copy_dir_recursive(&src_path, &dst_path).await?;
            } else if file_type.is_file() {
                tokio::fs::copy(&src_path, &dst_path)
                    .await
                    .map_err(|e| io_err(&src_path, e))?;
            } else if file_type.is_symlink() {
                let link_target = tokio::fs::read_link(&src_path)
                    .await
                    .map_err(|e| io_err(&src_path, e))?;
                tokio::fs::symlink(&link_target, &dst_path)
                    .await
                    .map_err(|e| io_err(&dst_path, e))?;
            }
        }

        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn directory_cow_name() {
        let cow = DirectoryCow;
        assert_eq!(cow.name(), "directory");
    }

    #[tokio::test]
    async fn directory_cow_is_available() {
        let cow = DirectoryCow;
        assert!(cow.is_available().await);
    }

    #[tokio::test]
    async fn snapshot_copies_files() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        let target = tmp.path().join("target");

        tokio::fs::create_dir_all(&source).await.unwrap();
        tokio::fs::write(source.join("hello.txt"), b"world")
            .await
            .unwrap();

        let cow = DirectoryCow;
        cow.snapshot(&source, &target).await.unwrap();

        let content = tokio::fs::read_to_string(target.join("hello.txt"))
            .await
            .unwrap();
        assert_eq!(content, "world");
    }

    #[tokio::test]
    async fn snapshot_copies_nested_directories() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        let nested = source.join("a").join("b");

        tokio::fs::create_dir_all(&nested).await.unwrap();
        tokio::fs::write(nested.join("deep.txt"), b"nested")
            .await
            .unwrap();

        let target = tmp.path().join("target");
        let cow = DirectoryCow;
        cow.snapshot(&source, &target).await.unwrap();

        let content = tokio::fs::read_to_string(target.join("a").join("b").join("deep.txt"))
            .await
            .unwrap();
        assert_eq!(content, "nested");
    }

    #[tokio::test]
    async fn snapshot_fails_if_source_missing() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("nonexistent");
        let target = tmp.path().join("target");

        let cow = DirectoryCow;
        let result = cow.snapshot(&source, &target).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn snapshot_fails_if_target_exists() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        let target = tmp.path().join("target");

        tokio::fs::create_dir_all(&source).await.unwrap();
        tokio::fs::create_dir_all(&target).await.unwrap();

        let cow = DirectoryCow;
        let result = cow.snapshot(&source, &target).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn snapshot_preserves_symlinks() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");

        tokio::fs::create_dir_all(&source).await.unwrap();
        tokio::fs::write(source.join("real.txt"), b"content")
            .await
            .unwrap();

        #[cfg(unix)]
        tokio::fs::symlink(source.join("real.txt"), source.join("link.txt"))
            .await
            .unwrap();

        let target = tmp.path().join("target");
        let cow = DirectoryCow;
        cow.snapshot(&source, &target).await.unwrap();

        let content = tokio::fs::read_to_string(target.join("link.txt"))
            .await
            .unwrap();
        assert_eq!(content, "content");
    }

    #[test]
    fn btrfs_cow_name() {
        let cow = BtrfsCow::new(SubvolumeMode::Off);
        assert_eq!(cow.name(), "btrfs");
    }

    #[tokio::test]
    async fn btrfs_is_not_available_on_non_linux() {
        let _cow = BtrfsCow::new(SubvolumeMode::Off);
        #[cfg(not(target_os = "linux"))]
        assert!(!_cow.is_available().await);
    }

    #[tokio::test]
    async fn is_btrfs_filesystem_returns_false_on_tempdir() {
        let tmp = TempDir::new().unwrap();
        assert!(!BtrfsCow::is_btrfs_filesystem(tmp.path()).await);
    }

    #[tokio::test]
    async fn detect_backend_returns_directory_on_non_btrfs() {
        let tmp = TempDir::new().unwrap();
        let backend = detect_backend(tmp.path()).await;
        assert_eq!(backend.name(), "directory");
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn btrfs_snapshot_fallback_on_non_subvolume() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        let target = tmp.path().join("target");

        tokio::fs::create_dir_all(&source).await.unwrap();
        tokio::fs::write(source.join("hello.txt"), b"world")
            .await
            .unwrap();

        let cow = BtrfsCow::new(SubvolumeMode::Off);
        cow.snapshot(&source, &target).await.unwrap();

        let content = tokio::fs::read_to_string(target.join("hello.txt"))
            .await
            .unwrap();
        assert_eq!(content, "world");
    }

    #[test]
    fn subvolume_mode_from_env_defaults_to_off() {
        // Ensure the variable is not set from a previous test.
        std::env::remove_var("ZEROCHAIN_BTRFS_SUBVOLUME_MODE");
        assert_eq!(SubvolumeMode::from_env(), SubvolumeMode::Off);
    }

    #[test]
    fn subvolume_mode_from_env_parses_variants() {
        for (value, expected) in [
            ("off", SubvolumeMode::Off),
            ("OFF", SubvolumeMode::Off),
            ("workflow", SubvolumeMode::Workflow),
            ("WORKFLOW", SubvolumeMode::Workflow),
            ("stage", SubvolumeMode::Stage),
            ("STAGE", SubvolumeMode::Stage),
            ("unknown", SubvolumeMode::Off),
        ] {
            std::env::set_var("ZEROCHAIN_BTRFS_SUBVOLUME_MODE", value);
            assert_eq!(SubvolumeMode::from_env(), expected, "failed for {value}");
        }
        std::env::remove_var("ZEROCHAIN_BTRFS_SUBVOLUME_MODE");
    }

    #[tokio::test]
    async fn btrfs_cow_prepare_stage_dir_falls_back_on_non_btrfs() {
        let tmp = TempDir::new().unwrap();
        let stage = tmp.path().join("stage");
        let cow = BtrfsCow::new(SubvolumeMode::Stage);
        cow.prepare_stage_dir(&stage).await.unwrap();
        assert!(stage.is_dir());
    }

    #[test]
    fn subvolume_mode_display_and_fromstr_round_trip() {
        for mode in [
            SubvolumeMode::Off,
            SubvolumeMode::Workflow,
            SubvolumeMode::Stage,
        ] {
            let s = mode.to_string();
            let parsed: SubvolumeMode = s.parse().unwrap();
            assert_eq!(parsed, mode);
        }
        assert!("unknown".parse::<SubvolumeMode>().is_err());
    }

    #[tokio::test]
    async fn subvolume_mode_save_and_load() {
        let tmp = TempDir::new().unwrap();
        SubvolumeMode::Stage.save(tmp.path()).await.unwrap();
        let loaded = SubvolumeMode::load(tmp.path()).await;
        assert_eq!(loaded, Some(SubvolumeMode::Stage));
    }

    #[tokio::test]
    async fn subvolume_mode_load_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(SubvolumeMode::load(tmp.path()).await, None);
    }

    #[tokio::test]
    async fn effective_mode_finds_saved_marker_by_walking_up() {
        let tmp = TempDir::new().unwrap();
        let workflow_root = tmp.path().join("wf");
        let stage = workflow_root.join("00_spec");
        tokio::fs::create_dir_all(&stage).await.unwrap();
        SubvolumeMode::Stage.save(&workflow_root).await.unwrap();

        let cow = BtrfsCow::new(SubvolumeMode::Off);
        let effective = cow.effective_mode(&stage).await;
        assert_eq!(effective, SubvolumeMode::Stage);
    }

    #[tokio::test]
    async fn effective_mode_falls_back_to_env_mode_when_no_marker() {
        let tmp = TempDir::new().unwrap();
        let workflow_root = tmp.path().join("wf");
        tokio::fs::create_dir_all(&workflow_root).await.unwrap();

        std::env::set_var("ZEROCHAIN_BTRFS_SUBVOLUME_MODE", "workflow");
        let cow = BtrfsCow::new(SubvolumeMode::from_env());
        let effective = cow.effective_mode(&workflow_root).await;
        assert_eq!(effective, SubvolumeMode::Workflow);
        std::env::remove_var("ZEROCHAIN_BTRFS_SUBVOLUME_MODE");
    }

    #[tokio::test]
    async fn delete_subvolume_or_dir_falls_back_on_plain_directory() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("plain");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("file.txt"), b"data").await.unwrap();

        let cow = BtrfsCow::new(SubvolumeMode::Off);
        cow.delete_subvolume_or_dir(&dir).await.unwrap();
        assert!(!dir.exists());
    }
}
