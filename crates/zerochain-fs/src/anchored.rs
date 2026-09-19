//! Dir-fd-anchored file reads and writes.
//!
//! These helpers close the TOCTOU window between a path-based containment
//! check and the actual open: the file is opened relative to a directory
//! file descriptor for the workspace root rather than re-resolving a full
//! path at open time. `cap-std` anchors resolution to that descriptor
//! (openat2 with `RESOLVE_BENEATH` on Linux, with portable fallbacks), so
//! a concurrent mutator swapping a parent directory for a symlink between
//! the check and the open cannot redirect the operation outside the root.
//!
//! Callers are still expected to perform their own up-front validation
//! (canonicalize-based containment, hard-link refusal) for error quality;
//! the anchoring here is the enforcement layer.

use std::io::Write;
use std::path::{Component, Path};

use crate::error::{io_err, FsError, Result};

/// Rejects relative paths that could escape the anchor: absolute paths and
/// any `..` component.
fn validate_rel(rel: &Path) -> Result<()> {
    if rel.is_absolute() {
        return Err(FsError::InvalidInput {
            message: format!("anchored path must be relative: {}", rel.display()),
        });
    }
    if rel
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(FsError::InvalidInput {
            message: format!("anchored path must not contain '..': {}", rel.display()),
        });
    }
    Ok(())
}

fn open_anchored(
    root: &Path,
    rel: &Path,
    options: &cap_std::fs::OpenOptions,
) -> Result<(std::path::PathBuf, cap_std::fs::File)> {
    validate_rel(rel)?;
    let dir = cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())
        .map_err(|e| io_err(root, e))?;
    let file = dir
        .open_with(rel, options)
        .map_err(|e| io_err(root.join(rel), e))?;
    Ok((root.join(rel), file))
}

/// Write `bytes` to `root`/`rel`, creating or truncating the file, with
/// path resolution anchored to the workspace root directory descriptor.
pub async fn write_anchored(root: &Path, rel: &Path, bytes: &[u8]) -> Result<()> {
    validate_rel(rel)?;
    let root = root.to_path_buf();
    let rel = rel.to_path_buf();
    let err_path = rel.clone();
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut options = cap_std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        let (path, mut file) = open_anchored(&root, &rel, &options)?;
        file.write_all(&bytes).map_err(|e| io_err(&path, e))?;
        file.flush().map_err(|e| io_err(&path, e))?;
        Ok(())
    })
    .await
    .map_err(|e| io_err(err_path, std::io::Error::other(e)))?
}

/// Read the contents of `root`/`rel` with path resolution anchored to the
/// workspace root directory descriptor.
pub async fn read_anchored(root: &Path, rel: &Path) -> Result<Vec<u8>> {
    validate_rel(rel)?;
    let root = root.to_path_buf();
    let rel = rel.to_path_buf();
    let err_path = rel.clone();
    tokio::task::spawn_blocking(move || {
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true);
        let (path, mut file) = open_anchored(&root, &rel, &options)?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut buf).map_err(|e| io_err(&path, e))?;
        Ok(buf)
    })
    .await
    .map_err(|e| io_err(err_path, std::io::Error::other(e)))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[tokio::test]
    async fn write_then_read_round_trips_inside_root() {
        let root = tempfile::tempdir().unwrap();

        write_anchored(root.path(), Path::new("file.txt"), b"hello anchored")
            .await
            .unwrap();
        let bytes = read_anchored(root.path(), Path::new("file.txt"))
            .await
            .unwrap();

        assert_eq!(bytes, b"hello anchored");
        assert_eq!(
            std::fs::read(root.path().join("file.txt")).unwrap(),
            b"hello anchored"
        );
    }

    #[tokio::test]
    async fn write_truncates_existing_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file.txt"), b"previous content").unwrap();

        write_anchored(root.path(), Path::new("file.txt"), b"new")
            .await
            .unwrap();

        assert_eq!(std::fs::read(root.path().join("file.txt")).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_escaping_root_is_refused_for_write_and_read() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "top secret").unwrap();
        std::os::unix::fs::symlink(&outside_file, root.path().join("escape")).unwrap();

        let write_err = write_anchored(root.path(), Path::new("escape"), b"clobbered")
            .await
            .expect_err("write through an escaping symlink must be refused");
        assert!(
            matches!(write_err, FsError::Io { .. }),
            "unexpected error: {write_err}"
        );

        let read_err = read_anchored(root.path(), Path::new("escape"))
            .await
            .expect_err("read through an escaping symlink must be refused");
        assert!(
            matches!(read_err, FsError::Io { .. }),
            "unexpected error: {read_err}"
        );

        // The outside file must be untouched.
        assert_eq!(
            std::fs::read_to_string(&outside_file).unwrap(),
            "top secret"
        );
    }

    #[tokio::test]
    async fn parent_dir_component_is_rejected_up_front() {
        let root = tempfile::tempdir().unwrap();

        let write_err = write_anchored(root.path(), Path::new("../escape.txt"), b"x")
            .await
            .expect_err("'..' in rel must be rejected");
        assert!(
            matches!(write_err, FsError::InvalidInput { .. }),
            "unexpected error: {write_err}"
        );

        let read_err = read_anchored(root.path(), Path::new("nested/../escape.txt"))
            .await
            .expect_err("'..' in rel must be rejected");
        assert!(
            matches!(read_err, FsError::InvalidInput { .. }),
            "unexpected error: {read_err}"
        );
    }

    #[tokio::test]
    async fn absolute_rel_is_rejected_up_front() {
        let root = tempfile::tempdir().unwrap();

        let err = read_anchored(root.path(), Path::new("/tmp/abs.txt"))
            .await
            .expect_err("absolute rel must be rejected");
        assert!(
            matches!(err, FsError::InvalidInput { .. }),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn missing_file_read_is_not_found_io_error() {
        let root = tempfile::tempdir().unwrap();

        let err = read_anchored(root.path(), Path::new("missing.txt"))
            .await
            .expect_err("missing file must error");
        match err {
            FsError::Io { source, .. } => assert_eq!(source.kind(), ErrorKind::NotFound),
            other => panic!("unexpected error: {other}"),
        }
    }
}
