use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use zerochain_error::{Result, ZerochainError};

use crate::tool::Tool;

fn workspace_and_target(input: &Value) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
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
                "path": { "type": "string", "description": "File path relative to the workspace root." },
                "content": { "type": "string", "description": "Content to write." }
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

        let parent = target.parent().unwrap_or(&workspace).to_path_buf();
        tokio::fs::create_dir_all(&parent)
            .await
            .map_err(|e| ZerochainError::Io {
                path: parent.clone(),
                source: e,
            })?;
        let canonical_parent = parent.canonicalize().map_err(|e| ZerochainError::Io {
            path: parent,
            source: e,
        })?;
        if !canonical_parent.starts_with(&workspace) {
            return Err(ZerochainError::InvalidInput {
                message: "path escapes workspace root".to_string(),
            });
        }

        tokio::fs::write(&target, content)
            .await
            .map_err(|e| ZerochainError::Io {
                path: target.clone(),
                source: e,
            })?;
        Ok(json!({ "written": true, "bytes": content.len() }))
    }
}
