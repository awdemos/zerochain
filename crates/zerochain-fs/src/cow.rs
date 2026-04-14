use std::path::Path;

use crate::error::{io_err, FsError, Result};

#[async_trait::async_trait]
pub trait CowPlatform: Send + Sync {
    async fn snapshot(&self, source_dir: &Path, target_dir: &Path) -> Result<()>;

    fn is_available(&self) -> bool;

    fn name(&self) -> &str;
}

pub struct DirectoryCow;

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
}
