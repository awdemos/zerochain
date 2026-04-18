use std::path::Path;

use crate::error::{io_err, FsError, Result};

#[async_trait::async_trait]
pub trait CowPlatform: Send + Sync {
    async fn snapshot(&self, source_dir: &Path, target_dir: &Path) -> Result<()>;

    fn is_available(&self) -> bool;

    fn name(&self) -> &str;
}

pub struct DirectoryCow;

pub struct BtrfsCow;

pub fn detect_backend(workspace_path: &Path) -> Box<dyn CowPlatform> {
    let btrfs = BtrfsCow;
    if btrfs.is_available() && BtrfsCow::is_btrfs_filesystem(workspace_path) {
        tracing::info!("CoW backend: btrfs (zero-copy snapshots)");
        Box::new(btrfs)
    } else {
        tracing::info!("CoW backend: directory (file-level copy)");
        Box::new(DirectoryCow)
    }
}

impl BtrfsCow {
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

        let stdout = child_proc.stdout.take().ok_or_else(|| {
            FsError::BtrfsCommandFailed {
                command: "send".into(),
                reason: "no stdout pipe".into(),
            }
        })?;

        let mut reader = tokio::io::BufReader::new(stdout);
        let mut file = tokio::fs::File::create(output_path).await.map_err(|e| {
            FsError::Io {
                path: output_path.to_path_buf(),
                source: e,
            }
        })?;

        tokio::io::copy(&mut reader, &mut file).await.map_err(|e| {
            FsError::BtrfsCommandFailed {
                command: "send".into(),
                reason: e.to_string(),
            }
        })?;

        let status = child_proc.wait().await.map_err(|e| {
            FsError::BtrfsCommandFailed {
                command: "send".into(),
                reason: e.to_string(),
            }
        })?;

        if !status.success() {
            return Err(FsError::BtrfsCommandFailed {
                command: "send".into(),
                reason: format!("exit code: {}", status.code().unwrap_or(-1)),
            });
        }
        Ok(())
    }

    pub fn is_btrfs_filesystem(path: &Path) -> bool {
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("stat")
                .args(["-f", "--format", "%T", &path.to_string_lossy()])
                .output()
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
            return Err(FsError::SnapshotFailed {
                src_path: source_dir.to_path_buf(),
                target: target_dir.to_path_buf(),
                reason: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        tracing::debug!(
            source = %source_dir.display(),
            target = %target_dir.display(),
            "btrfs snapshot created"
        );

        Ok(())
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("btrfs")
            .arg("--version")
            .output()
            .is_ok()
    }

    fn name(&self) -> &str {
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

        copy_dir_recursive(source_dir, target_dir).await?;

        tracing::debug!(
            source = %source_dir.display(),
            target = %target_dir.display(),
            "directory snapshot created"
        );

        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
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

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| io_err(source, e))?
        {
            let src_path = entry.path();
            let file_name = entry.file_name();
            let dst_path = target.join(&file_name);

            let file_type = entry
                .file_type()
                .await
                .map_err(|e| io_err(&src_path, e))?;

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
        assert!(cow.is_available());
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
        let cow = BtrfsCow;
        assert_eq!(cow.name(), "btrfs");
    }

    #[test]
    fn btrfs_is_not_available_on_non_linux() {
        let cow = BtrfsCow;
        #[cfg(not(target_os = "linux"))]
        assert!(!cow.is_available());
    }

    #[test]
    fn is_btrfs_filesystem_returns_false_on_tempdir() {
        let tmp = TempDir::new().unwrap();
        assert!(!BtrfsCow::is_btrfs_filesystem(tmp.path()));
    }

    #[test]
    fn detect_backend_returns_directory_on_non_btrfs() {
        let tmp = TempDir::new().unwrap();
        let backend = detect_backend(tmp.path());
        assert_eq!(backend.name(), "directory");
    }
}
