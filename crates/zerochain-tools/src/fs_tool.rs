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
