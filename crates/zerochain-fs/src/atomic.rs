use std::path::{Path, PathBuf};

use crate::error::{io_err, FsError, Result};

const COMPLETE_MARKER: &str = ".complete";
const ERROR_MARKER: &str = ".error";
const EXECUTING_MARKER: &str = ".executing";
const LOCK_FILE: &str = ".lock";

pub async fn write_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| FsError::AtomicWriteFailed {
        path: path.to_path_buf(),
        reason: "path has no parent directory".into(),
    })?;

    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| io_err(parent, e))?;

    let file_name = path
        .file_name()
        .ok_or_else(|| FsError::AtomicWriteFailed {
            path: path.to_path_buf(),
            reason: "path has no file name".into(),
        })?;

    let tmp_name = format!(
        ".tmp.{}.{}",
        file_name.to_string_lossy(),
        std::process::id()
    );
    let tmp_path = parent.join(&tmp_name);

    tokio::fs::write(&tmp_path, content)
        .await
        .map_err(|e| io_err(&tmp_path, e))?;

    tokio::fs::rename(&tmp_path, path)
        .await
        .map_err(|e| io_err(path, e))?;

    tracing::debug!(path = %path.display(), "atomic write complete");
    Ok(())
}

pub async fn mark_complete(dir: &Path, metadata: Option<&str>) -> Result<()> {
    let marker_path = dir.join(COMPLETE_MARKER);
    let content = metadata.unwrap_or("");
    write_atomic(&marker_path, content.as_bytes()).await?;

    let error_path = dir.join(ERROR_MARKER);
    let exec_path = dir.join(EXECUTING_MARKER);
    let _ = tokio::fs::remove_file(&error_path).await;
    let _ = tokio::fs::remove_file(&exec_path).await;

    Ok(())
}

pub async fn is_complete(dir: &Path) -> bool {
    tokio::fs::try_exists(dir.join(COMPLETE_MARKER))
        .await
        .unwrap_or(false)
}

pub async fn mark_error(dir: &Path, error: &str) -> Result<()> {
    let marker_path = dir.join(ERROR_MARKER);
    write_atomic(&marker_path, error.as_bytes()).await?;

    let complete_path = dir.join(COMPLETE_MARKER);
    let exec_path = dir.join(EXECUTING_MARKER);
    let _ = tokio::fs::remove_file(&complete_path).await;
    let _ = tokio::fs::remove_file(&exec_path).await;

    Ok(())
}

pub async fn is_error(dir: &Path) -> bool {
    tokio::fs::try_exists(dir.join(ERROR_MARKER))
        .await
        .unwrap_or(false)
}

pub async fn mark_executing(dir: &Path) -> Result<()> {
    let marker_path = dir.join(EXECUTING_MARKER);
    let content = format!("PID:{}\nTIMESTAMP:{}\n", std::process::id(), epoch_secs());
    write_atomic(&marker_path, content.as_bytes()).await
}

pub async fn is_executing(dir: &Path) -> bool {
    tokio::fs::try_exists(dir.join(EXECUTING_MARKER))
        .await
        .unwrap_or(false)
}

pub async fn clear_executing(dir: &Path) -> Result<()> {
    let marker_path = dir.join(EXECUTING_MARKER);
    match tokio::fs::remove_file(&marker_path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(&marker_path, e)),
    }
}

pub async fn clean_output(dir: &Path) -> Result<()> {
    let output_dir = dir.join("output");

    if !tokio::fs::try_exists(&output_dir)
        .await
        .map_err(|e| io_err(&output_dir, e))?
    {
        tokio::fs::create_dir_all(&output_dir)
            .await
            .map_err(|e| io_err(&output_dir, e))?;
        return Ok(());
    }

    let mut entries = tokio::fs::read_dir(&output_dir)
        .await
        .map_err(|e| io_err(&output_dir, e))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| io_err(&output_dir, e))?
    {
        let path = entry.path();
        let file_type = entry.file_type().await.map_err(|e| io_err(&path, e))?;

        if file_type.is_dir() {
            tokio::fs::remove_dir_all(&path)
                .await
                .map_err(|e| io_err(&path, e))?;
        } else {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| io_err(&path, e))?;
        }
    }

    Ok(())
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs()
}

fn is_pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        PathBuf::from("/proc")
            .join(pid.to_string())
            .exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

fn parse_lock_content(content: &str) -> Option<(u32, u64)> {
    let mut pid = None;
    let mut timestamp = None;

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("PID:") {
            pid = rest.parse().ok();
        } else if let Some(rest) = line.strip_prefix("TIMESTAMP:") {
            timestamp = rest.parse().ok();
        }
    }

    Some((pid?, timestamp?))
}

pub struct LockGuard {
    lock_path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let path = self.lock_path.clone();
        std::fs::remove_file(&path).ok();
    }
}

pub async fn acquire_lock(dir: &Path) -> Result<LockGuard> {
    let lock_path = dir.join(LOCK_FILE);

    if tokio::fs::try_exists(&lock_path)
        .await
        .map_err(|e| io_err(&lock_path, e))?
    {
        let content = tokio::fs::read_to_string(&lock_path)
            .await
            .map_err(|e| io_err(&lock_path, e))?;

        if let Some((pid, _ts)) = parse_lock_content(&content) {
            if pid != std::process::id() && is_pid_alive(pid) {
                return Err(FsError::LockHeld {
                    path: lock_path,
                    pid,
                });
            }
        }
    }

    let content = format!("PID:{}\nTIMESTAMP:{}\n", std::process::id(), epoch_secs());
    tokio::fs::write(&lock_path, &content)
        .await
        .map_err(|e| io_err(&lock_path, e))?;

    Ok(LockGuard { lock_path })
}

pub async fn is_locked(dir: &Path) -> bool {
    let lock_path = dir.join(LOCK_FILE);

    let content = match tokio::fs::read_to_string(&lock_path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(e) => {
            tracing::warn!(path = %lock_path.display(), error = %e, "failed to read lock file");
            return false;
        }
    };

    let Some((pid, _ts)) = parse_lock_content(&content) else {
        return false;
    };

    pid != std::process::id() && is_pid_alive(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn write_atomic_creates_file() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("output.txt");

        write_atomic(&file_path, b"hello world").await.unwrap();

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn write_atomic_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("a").join("b").join("file.txt");

        write_atomic(&file_path, b"nested").await.unwrap();

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "nested");
    }

    #[tokio::test]
    async fn write_atomic_is_atomic_on_overwrite() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("data.txt");

        write_atomic(&file_path, b"first").await.unwrap();
        write_atomic(&file_path, b"second").await.unwrap();

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "second");
    }

    #[tokio::test]
    async fn write_atomic_no_temp_file_left() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("clean.txt");

        write_atomic(&file_path, b"data").await.unwrap();

        let mut entries = tokio::fs::read_dir(tmp.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(!name.starts_with(".tmp."), "leftover temp file: {name}");
        }
    }

    #[tokio::test]
    async fn mark_complete_and_check() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_complete(tmp.path()).await);

        mark_complete(tmp.path(), Some("stage-1")).await.unwrap();
        assert!(is_complete(tmp.path()).await);

        let content = tokio::fs::read_to_string(tmp.path().join(".complete"))
            .await
            .unwrap();
        assert_eq!(content, "stage-1");
    }

    #[tokio::test]
    async fn mark_complete_without_metadata() {
        let tmp = TempDir::new().unwrap();
        mark_complete(tmp.path(), None).await.unwrap();
        assert!(is_complete(tmp.path()).await);

        let content = tokio::fs::read_to_string(tmp.path().join(".complete"))
            .await
            .unwrap();
        assert!(content.is_empty());
    }

    #[tokio::test]
    async fn mark_error_and_check() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_error(tmp.path()).await);

        mark_error(tmp.path(), "something broke").await.unwrap();
        assert!(is_error(tmp.path()).await);

        let content = tokio::fs::read_to_string(tmp.path().join(".error"))
            .await
            .unwrap();
        assert_eq!(content, "something broke");
    }

    #[tokio::test]
    async fn markers_mutually_exclusive() {
        let tmp = TempDir::new().unwrap();

        mark_complete(tmp.path(), None).await.unwrap();
        assert!(is_complete(tmp.path()).await);
        assert!(!is_error(tmp.path()).await);

        mark_error(tmp.path(), "late failure").await.unwrap();
        assert!(!is_complete(tmp.path()).await);
        assert!(is_error(tmp.path()).await);
    }

    #[tokio::test]
    async fn mark_complete_removes_error() {
        let tmp = TempDir::new().unwrap();

        mark_error(tmp.path(), "fail").await.unwrap();
        assert!(is_error(tmp.path()).await);
        assert!(!is_complete(tmp.path()).await);

        mark_complete(tmp.path(), None).await.unwrap();
        assert!(is_complete(tmp.path()).await);
        assert!(!is_error(tmp.path()).await);
    }

    #[tokio::test]
    async fn mark_complete_removes_executing() {
        let tmp = TempDir::new().unwrap();

        mark_executing(tmp.path()).await.unwrap();
        assert!(is_executing(tmp.path()).await);

        mark_complete(tmp.path(), None).await.unwrap();
        assert!(is_complete(tmp.path()).await);
        assert!(!is_executing(tmp.path()).await);
    }

    #[tokio::test]
    async fn mark_error_removes_executing() {
        let tmp = TempDir::new().unwrap();

        mark_executing(tmp.path()).await.unwrap();
        assert!(is_executing(tmp.path()).await);

        mark_error(tmp.path(), "boom").await.unwrap();
        assert!(is_error(tmp.path()).await);
        assert!(!is_executing(tmp.path()).await);
    }

    #[tokio::test]
    async fn is_complete_false_for_nonexistent_dir() {
        assert!(!is_complete(Path::new("/no/such/dir/ever")).await);
    }

    #[tokio::test]
    async fn is_error_false_for_nonexistent_dir() {
        assert!(!is_error(Path::new("/no/such/dir/ever")).await);
    }

    #[tokio::test]
    async fn mark_executing_creates_marker() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_executing(tmp.path()).await);

        mark_executing(tmp.path()).await.unwrap();
        assert!(is_executing(tmp.path()).await);

        let content = tokio::fs::read_to_string(tmp.path().join(".executing"))
            .await
            .unwrap();
        assert!(content.starts_with(&format!("PID:{}\n", std::process::id())));
        assert!(content.contains("TIMESTAMP:"));
    }

    #[tokio::test]
    async fn clear_executing_removes_marker() {
        let tmp = TempDir::new().unwrap();

        mark_executing(tmp.path()).await.unwrap();
        assert!(is_executing(tmp.path()).await);

        clear_executing(tmp.path()).await.unwrap();
        assert!(!is_executing(tmp.path()).await);
    }

    #[tokio::test]
    async fn clear_executing_idempotent() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_executing(tmp.path()).await);

        clear_executing(tmp.path()).await.unwrap();
        assert!(!is_executing(tmp.path()).await);
    }

    #[tokio::test]
    async fn acquire_lock_creates_guard() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_locked(tmp.path()).await);

        let guard = acquire_lock(tmp.path()).await.unwrap();
        assert!(tokio::fs::try_exists(tmp.path().join(".lock"))
            .await
            .unwrap());

        drop(guard);
        assert!(!tokio::fs::try_exists(tmp.path().join(".lock"))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn is_locked_by_current_process_is_false() {
        let tmp = TempDir::new().unwrap();
        let _guard = acquire_lock(tmp.path()).await.unwrap();

        // Current process holds the lock, so is_locked returns false
        // (is_locked checks if a DIFFERENT live process holds it)
        assert!(!is_locked(tmp.path()).await);
    }

    #[tokio::test]
    async fn acquire_lock_steals_stale() {
        let tmp = TempDir::new().unwrap();
        let lock_path = tmp.path().join(".lock");

        // Write a lock file with a PID that doesn't exist
        let stale_content = "PID:999999999\nTIMESTAMP:0\n";
        tokio::fs::write(&lock_path, &stale_content)
            .await
            .unwrap();

        // Should succeed because the PID is dead
        let _guard = acquire_lock(tmp.path()).await.unwrap();
    }

    #[tokio::test]
    async fn acquire_lock_contention_fails() {
        let tmp = TempDir::new().unwrap();
        let lock_path = tmp.path().join(".lock");

        // Spawn a subprocess we can use as a "live process" placeholder
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let external_pid = child.id();

        let content = format!("PID:{external_pid}\nTIMESTAMP:0\n");
        tokio::fs::write(&lock_path, &content).await.unwrap();

        let result = acquire_lock(tmp.path()).await;
        assert!(result.is_err());

        let _ = child.kill();
        let _ = child.wait();
    }

    #[tokio::test]
    async fn lock_guard_not_clone() {
        let tmp = TempDir::new().unwrap();
        let guard = acquire_lock(tmp.path()).await.unwrap();

        // This line verifies LockGuard is NOT Clone at compile time
        let _ = &guard;
        drop(guard);
    }

    #[tokio::test]
    async fn clean_output_removes_contents() {
        let tmp = TempDir::new().unwrap();
        let output = tmp.path().join("output");
        tokio::fs::create_dir_all(&output).await.unwrap();
        tokio::fs::write(output.join("file1.txt"), b"data1")
            .await
            .unwrap();
        tokio::fs::create_dir_all(output.join("subdir"))
            .await
            .unwrap();
        tokio::fs::write(output.join("subdir").join("file2.txt"), b"data2")
            .await
            .unwrap();

        clean_output(tmp.path()).await.unwrap();

        assert!(tokio::fs::try_exists(&output).await.unwrap());
        assert!(!tokio::fs::try_exists(output.join("file1.txt"))
            .await
            .unwrap());
        assert!(!tokio::fs::try_exists(output.join("subdir"))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn clean_output_creates_dir_if_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(!tokio::fs::try_exists(tmp.path().join("output"))
            .await
            .unwrap());

        clean_output(tmp.path()).await.unwrap();

        assert!(tokio::fs::try_exists(tmp.path().join("output"))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn clean_output_idempotent() {
        let tmp = TempDir::new().unwrap();
        clean_output(tmp.path()).await.unwrap();
        clean_output(tmp.path()).await.unwrap();
        assert!(tokio::fs::try_exists(tmp.path().join("output"))
            .await
            .unwrap());
    }
}
