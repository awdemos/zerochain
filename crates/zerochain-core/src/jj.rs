use std::path::Path;

use crate::error::{Error, Result};

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
            .map_err(|_| Error::JjNotInstalled)?;

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
            .map_err(|_| Error::JjNotInstalled)?;

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
            .map_err(|_| Error::JjNotInstalled)?;

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
            .map_err(|_| Error::JjNotInstalled)?;

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
