use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use zerochain_error::{Result, ZerochainError};

use crate::tool::Tool;

fn workspace_and_target(input: &Value) -> Result<(PathBuf, PathBuf)> {
    let path =
        input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ZerochainError::InvalidInput {
                message: "missing 'path' field".to_string(),
            })?;
    let workspace_root = input
        .get("workspace_root")
        .and_then(Value::as_str)
        .ok_or_else(|| ZerochainError::InvalidInput {
            message: "missing 'workspace_root' field".to_string(),
        })?;

    let root = Path::new(workspace_root)
        .canonicalize()
        .map_err(|e| ZerochainError::Io {
            path: Path::new(workspace_root).to_path_buf(),
            source: e,
        })?;
    Ok((root.clone(), root.join(path)))
}

/// Ensures the parent directory of `target` exists inside `workspace`, creating
/// missing directories component-by-component and verifying containment at each step.
async fn ensure_parent_in_workspace(workspace: &Path, target: &Path) -> Result<PathBuf> {
    let parent = target.parent().unwrap_or(workspace).to_path_buf();

    // If the parent already exists, canonicalize and verify containment.
    if let Ok(canonical) = parent.canonicalize() {
        if !canonical.starts_with(workspace) {
            return Err(ZerochainError::InvalidInput {
                message: "path escapes workspace root".to_string(),
            });
        }
        return Ok(canonical);
    }

    // Build the parent path component-by-component so that any symlink that
    // escapes the workspace is caught before a directory is created.
    let relative = parent
        .strip_prefix(workspace)
        .map_err(|_| ZerochainError::InvalidInput {
            message: "path escapes workspace root".to_string(),
        })?;

    let mut current = workspace.to_path_buf();
    for component in relative.components() {
        let next = current.join(component);
        match next.canonicalize() {
            Ok(canonical) => {
                if !canonical.starts_with(workspace) {
                    return Err(ZerochainError::InvalidInput {
                        message: "path escapes workspace root".to_string(),
                    });
                }
                current = canonical;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::create_dir(&next)
                    .await
                    .map_err(|e| ZerochainError::Io {
                        path: next.clone(),
                        source: e,
                    })?;
                let canonical = next.canonicalize().map_err(|e| ZerochainError::Io {
                    path: next,
                    source: e,
                })?;
                if !canonical.starts_with(workspace) {
                    return Err(ZerochainError::InvalidInput {
                        message: "path escapes workspace root".to_string(),
                    });
                }
                current = canonical;
            }
            Err(e) => {
                return Err(ZerochainError::Io {
                    path: next,
                    source: e,
                });
            }
        }
    }

    Ok(current)
}

/// Refuses to operate on a pre-existing file that has more than one hard link.
///
/// The containment checks in this module are canonicalize-based: a hard link
/// created inside the workspace shares an inode with a file anywhere on the
/// filesystem, so reading through it exfiltrates outside data and writing
/// through it truncates an outside file while every path check passes.
///
/// Residual risk: there is a TOCTOU window between this check and the actual
/// read/write during which the path can be swapped; closing that requires
/// openat2(RESOLVE_BENEATH)-style resolution, which std/tokio do not expose.
fn ensure_not_hardlinked(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path).map_err(|e| ZerochainError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Err(ZerochainError::InvalidInput {
                message: format!(
                    "refusing to access file with multiple hard links: {}",
                    path.display()
                ),
            });
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    Ok(())
}

/// Read a file relative to the workspace root.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file relative to the workspace root."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the workspace root."
                },
                "workspace_root": {
                    "type": "string",
                    "description": "Injected by the engine; do not set manually."
                }
            },
            "required": ["path"]
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let (workspace, target) = workspace_and_target(&input)?;

        match target.canonicalize() {
            Ok(canonical) => {
                if !canonical.starts_with(&workspace) {
                    return Err(ZerochainError::InvalidInput {
                        message: "path escapes workspace root".to_string(),
                    });
                }
                ensure_not_hardlinked(&canonical)?;
                let content = tokio::fs::read_to_string(&canonical).await.map_err(|e| {
                    ZerochainError::Io {
                        path: canonical,
                        source: e,
                    }
                })?;
                Ok(json!({ "content": content, "exists": true }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(json!({ "content": "", "exists": false }))
            }
            Err(e) => Err(ZerochainError::Io {
                path: target,
                source: e,
            }),
        }
    }
}

/// Write content to a file relative to the workspace root.
#[derive(Clone, Copy, Debug, Default)]
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file relative to the workspace root, creating parent directories."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the workspace root."
                },
                "content": {
                    "type": "string",
                    "description": "Content to write."
                },
                "workspace_root": {
                    "type": "string",
                    "description": "Injected by the engine; do not set manually."
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let content = input
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| ZerochainError::InvalidInput {
                message: "missing 'content' field".to_string(),
            })?;
        let (workspace, target) = workspace_and_target(&input)?;

        // Verify (and create) the parent directory inside the workspace before any
        // filesystem mutation that could follow an escaping symlink.
        ensure_parent_in_workspace(&workspace, &target).await?;

        // If the target already exists, canonicalize it to catch a symlink that
        // points outside the workspace.
        if let Ok(canonical) = target.canonicalize() {
            if !canonical.starts_with(&workspace) {
                return Err(ZerochainError::InvalidInput {
                    message: "path escapes workspace root".to_string(),
                });
            }
            ensure_not_hardlinked(&canonical)?;
        }

        tokio::fs::write(&target, content)
            .await
            .map_err(|e| ZerochainError::Io {
                path: target.clone(),
                source: e,
            })?;

        // Final verification: the written file must resolve inside the workspace.
        let canonical = target.canonicalize().map_err(|e| ZerochainError::Io {
            path: target.clone(),
            source: e,
        })?;
        if !canonical.starts_with(&workspace) {
            return Err(ZerochainError::InvalidInput {
                message: "path escapes workspace root".to_string(),
            });
        }

        Ok(json!({ "written": true, "bytes": content.len() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Hard links are not supported on every filesystem; callers skip the
    /// assertion when the link cannot be created.
    fn try_hard_link(link: &Path, original: &Path) -> bool {
        std::fs::hard_link(original, link).is_ok()
    }

    #[tokio::test]
    async fn write_file_refuses_hard_linked_target() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        let inside = workspace.path().join("linked.txt");
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "original secret").unwrap();
        if !try_hard_link(&inside, &outside_file) {
            return;
        }

        let tool = WriteFileTool;
        let err = tool
            .run(json!({
                "path": "linked.txt",
                "content": "clobbered",
                "workspace_root": workspace.path().to_str().unwrap(),
            }))
            .await
            .expect_err("write through a hard link must be refused");
        assert!(
            matches!(err, ZerochainError::InvalidInput { .. }),
            "unexpected error: {err}"
        );

        // The outside file must not have been truncated.
        assert_eq!(
            std::fs::read_to_string(&outside_file).unwrap(),
            "original secret"
        );
    }

    #[tokio::test]
    async fn read_file_refuses_hard_linked_target() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        let inside = workspace.path().join("linked.txt");
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "outside secret").unwrap();
        if !try_hard_link(&inside, &outside_file) {
            return;
        }

        let tool = ReadFileTool;
        let err = tool
            .run(json!({
                "path": "linked.txt",
                "workspace_root": workspace.path().to_str().unwrap(),
            }))
            .await
            .expect_err("read through a hard link must be refused");
        assert!(
            matches!(err, ZerochainError::InvalidInput { .. }),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn write_file_allows_regular_files() {
        let workspace = tempfile::tempdir().unwrap();

        let tool = WriteFileTool;
        let result = tool
            .run(json!({
                "path": "sub/dir/file.txt",
                "content": "hello",
                "workspace_root": workspace.path().to_str().unwrap(),
            }))
            .await
            .unwrap();
        assert!(result.get("written").unwrap().as_bool().unwrap());
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("sub/dir/file.txt")).unwrap(),
            "hello"
        );
    }
}
