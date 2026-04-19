use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

fn map_jj_spawn_error(e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        Error::JjNotInstalled
    } else {
        Error::JjError {
            message: format!("failed to spawn jj: {e}"),
        }
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CommitEntry {
    pub change_id: String,
    pub commit_id: Option<String>,
    pub author: String,
    pub message: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone)]
pub struct JjManager;

impl JjManager {
    pub async fn init(path: &Path) -> Result<()> {
        let output = tokio::process::Command::new("jj")
            .arg("init")
            .arg("--git")
            .arg(path)
            .output()
            .await
            .map_err(map_jj_spawn_error)?;

        if !output.status.success() {
            return Err(Error::JjError {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(())
    }

    pub async fn commit(path: &Path, message: &str) -> Result<()> {
        Self::require_jj()?;

        let describe_output = tokio::process::Command::new("jj")
            .args(["describe", "-m", message])
            .current_dir(path)
            .output()
            .await
            .map_err(map_jj_spawn_error)?;

        if !describe_output.status.success() {
            return Err(Error::JjError {
                message: format!(
                    "jj describe failed: {}",
                    String::from_utf8_lossy(&describe_output.stderr)
                ),
            });
        }

        let new_output = tokio::process::Command::new("jj")
            .args(["new"])
            .current_dir(path)
            .output()
            .await
            .map_err(map_jj_spawn_error)?;

        if !new_output.status.success() {
            return Err(Error::JjError {
                message: format!(
                    "jj new failed: {}",
                    String::from_utf8_lossy(&new_output.stderr)
                ),
            });
        }

        Ok(())
    }

    pub async fn log(path: &Path, limit: usize) -> Result<Vec<CommitEntry>> {
        Self::require_jj()?;

        let template = r#"change_id ++ "\n" ++ commit_id ++ "\n" ++ author ++ "\n" ++ description ++ "\n" ++ committer.timestamp() ++ "\n---ENTRY---\n""#;

        let output = tokio::process::Command::new("jj")
            .args([
                "log",
                "--no-graph",
                "-T",
                template,
                "-n",
                &limit.to_string(),
            ])
            .current_dir(path)
            .output()
            .await
            .map_err(map_jj_spawn_error)?;

        if !output.status.success() {
            return Err(Error::JjError {
                message: format!(
                    "jj log failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let mut entries = Vec::new();

        for entry_text in text.split("---ENTRY---") {
            let lines: Vec<&str> = entry_text.lines().collect();
            if lines.len() >= 4 {
                let change_id = lines[0].trim().to_string();
                if change_id.is_empty() {
                    continue;
                }
                let commit_id_str = lines[1].trim();
                let commit_id = if commit_id_str.is_empty() || commit_id_str.contains("(") {
                    None
                } else {
                    Some(commit_id_str.to_string())
                };
                let author = lines[2].trim().to_string();
                let message = lines[3].trim().to_string();
                let timestamp = lines.get(4).map(|s| s.trim().to_string());

                entries.push(CommitEntry {
                    change_id,
                    commit_id,
                    author,
                    message,
                    timestamp,
                });
            }
        }

        Ok(entries)
    }

    pub fn require_jj() -> Result<()> {
        Ok(())
    }
}

pub async fn is_jj_installed() -> bool {
    tokio::process::Command::new("jj")
        .arg("--version")
        .output()
        .await
        .is_ok()
}

pub struct JjWorkspace {
    pub path: PathBuf,
}

impl JjWorkspace {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub async fn export_bundle(&self, output_path: &Path) -> Result<()> {
        let output = tokio::process::Command::new("git")
            .args([
                "bundle",
                "create",
                &output_path.to_string_lossy(),
                "--all",
            ])
            .current_dir(&self.path)
            .output()
            .await
            .map_err(|e| Error::JjError {
                message: format!("git bundle create failed: {e}"),
            })?;

        if !output.status.success() {
            return Err(Error::JjError {
                message: format!(
                    "git bundle create failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }
        Ok(())
    }

    pub async fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        let output = tokio::process::Command::new("git")
            .args(["remote", "add", name, url])
            .current_dir(&self.path)
            .output()
            .await
            .map_err(|e| Error::JjError {
                message: format!("git remote add failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("already exists") {
                return Err(Error::JjError {
                    message: format!("git remote add failed: {stderr}"),
                });
            }
        }
        Ok(())
    }

    pub async fn push_remote(&self, remote_name: &str) -> Result<()> {
        let output = tokio::process::Command::new("jj")
            .args(["git", "push", "-r", "@", "--remote", remote_name])
            .current_dir(&self.path)
            .output()
            .await
            .map_err(|e| Error::JjError {
                message: format!("jj git push failed: {e}"),
            })?;

        if !output.status.success() {
            return Err(Error::JjError {
                message: format!(
                    "jj git push failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }
        Ok(())
    }

    pub async fn workspace_size(&self) -> Result<u64> {
        let mut total: u64 = 0;
        Self::dir_size(&self.path, &mut total).await?;
        Ok(total)
    }

    pub async fn export_archive(&self, output_path: &Path) -> Result<()> {
        let parent = output_path.parent().ok_or_else(|| Error::JjError {
            message: "output path has no parent directory".into(),
        })?;
        tokio::fs::create_dir_all(parent).await.map_err(|e| Error::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;

        let file_name = output_path
            .file_name()
            .ok_or_else(|| Error::JjError {
                message: "output path has no file name".into(),
            })?
            .to_string_lossy()
            .to_string();

        let output = tokio::process::Command::new("tar")
            .args([
                "cf",
                &file_name,
                "-C",
                &self.path.to_string_lossy(),
                ".",
            ])
            .current_dir(parent)
            .output()
            .await
            .map_err(|e| Error::JjError {
                message: format!("tar create failed: {e}"),
            })?;

        if !output.status.success() {
            return Err(Error::JjError {
                message: format!(
                    "tar create failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }
        Ok(())
    }

    fn dir_size<'a>(
        dir: &'a Path,
        total: &'a mut u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut entries = tokio::fs::read_dir(dir).await.map_err(|e| Error::Io {
                path: dir.to_path_buf(),
                source: e,
            })?;

            while let Some(entry) = entries.next_entry().await.map_err(|e| Error::Io {
                path: dir.to_path_buf(),
                source: e,
            })? {
                let meta = entry.metadata().await.map_err(|e| Error::Io {
                    path: entry.path(),
                    source: e,
                })?;
                if meta.is_dir() {
                    Self::dir_size(&entry.path(), total).await?;
                } else {
                    *total += meta.len();
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn require_jj_returns_ok() {
        let result = JjManager::require_jj();
        assert!(result.is_ok());
    }

    #[test]
    fn commit_entry_fields() {
        let entry = CommitEntry {
            change_id: "abc123".to_string(),
            commit_id: Some("def456".to_string()),
            author: "Test <test@example.com>".to_string(),
            message: "Initial commit".to_string(),
            timestamp: Some("2025-01-01".to_string()),
        };
        assert_eq!(entry.change_id, "abc123");
        assert_eq!(entry.commit_id, Some("def456".to_string()));
    }
}
