use std::cmp::Ordering;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::context::Context;
use crate::error::{Error, Result};

/// Stage identifier parsed from `NN_name` directory format (e.g., `01_analyze`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StageId {
    pub sequence: u32,
    pub name: String,
    pub raw: String,
}

impl StageId {
    pub fn parse(dir_name: &str) -> Result<Self> {
        let underscore_pos = dir_name.find('_').ok_or_else(|| Error::InvalidStageName {
            name: dir_name.to_string(),
        })?;

        let prefix = &dir_name[..underscore_pos];
        let suffix = &dir_name[underscore_pos + 1..];

        if suffix.is_empty() {
            return Err(Error::InvalidStageName {
                name: dir_name.to_string(),
            });
        }

        let numeric_end = prefix
            .chars()
            .take_while(char::is_ascii_digit)
            .count();

        if numeric_end == 0 {
            return Err(Error::InvalidStageName {
                name: dir_name.to_string(),
            });
        }

        let sequence: u32 = prefix[..numeric_end]
            .parse()
            .map_err(|_| Error::InvalidStageName {
                name: dir_name.to_string(),
            })?;

        let letter = &prefix[numeric_end..];
        if !letter.is_empty()
            && (letter.len() != 1 || !letter.chars().next().unwrap().is_ascii_lowercase())
        {
            return Err(Error::InvalidStageName {
                name: dir_name.to_string(),
            });
        }

        Ok(StageId {
            sequence,
            name: format!("{letter}{suffix}"),
            raw: dir_name.to_string(),
        })
    }

    #[must_use] pub fn parallel_group(&self) -> Option<String> {
        let numeric_end = self
            .raw
            .chars()
            .take_while(char::is_ascii_digit)
            .count();
        let rest = &self.raw[numeric_end..];
        if rest.starts_with('_') {
            return None;
        }
        rest.chars()
            .next()
            .filter(char::is_ascii_lowercase)
            .map(|c| c.to_string())
    }

    #[must_use] pub fn sort_key(&self) -> (u32, String) {
        (
            self.sequence,
            self.raw.split('_').next().unwrap_or(&self.raw).to_string(),
        )
    }
}

impl Ord for StageId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl PartialOrd for StageId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for StageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

/// A single stage within a workflow, parsed from a `NN_name/` directory.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Stage {
    pub id: StageId,
    pub path: PathBuf,
    pub context_path: PathBuf,
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub is_complete: bool,
    pub is_error: bool,
    pub human_gate: bool,
    pub container_image: Option<String>,
    pub command: Option<String>,
}

impl Stage {
    pub async fn from_dir(dir: &std::path::Path) -> Result<Self> {
        let dir_name = dir
            .file_name()
            .ok_or_else(|| Error::InvalidStageName {
                name: dir.to_string_lossy().to_string(),
            })?
            .to_string_lossy()
            .to_string();

        let id = StageId::parse(&dir_name)?;

        let context_path = dir.join("CONTEXT.md");
        let input_path = dir.join("input");
        let output_path = dir.join("output");

        let is_complete = tokio::fs::try_exists(dir.join(".complete"))
            .await
            .unwrap_or(false);
        let is_error = tokio::fs::try_exists(dir.join(".error"))
            .await
            .unwrap_or(false);

        let (human_gate, container_image, command) =
            if tokio::fs::try_exists(&context_path).await.unwrap_or(false) {
                let ctx = Context::from_file(&context_path).await?;
                (
                    ctx.frontmatter.human_gate,
                    ctx.frontmatter.container,
                    ctx.frontmatter.command,
                )
            } else {
                (false, None, None)
            };

        Ok(Stage {
            id,
            path: dir.to_path_buf(),
            context_path,
            input_path,
            output_path,
            is_complete,
            is_error,
            human_gate,
            container_image,
            command,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stage_id_basic() {
        let id = StageId::parse("01_analyze").unwrap();
        assert_eq!(id.sequence, 1);
        assert_eq!(id.name, "analyze");
        assert_eq!(id.raw, "01_analyze");
        assert!(id.parallel_group().is_none());
    }

    #[test]
    fn parse_stage_id_two_digits() {
        let id = StageId::parse("12_deploy_prod").unwrap();
        assert_eq!(id.sequence, 12);
        assert_eq!(id.name, "deploy_prod");
    }

    #[test]
    fn parse_stage_id_with_parallel_suffix() {
        let id = StageId::parse("02a_test_unit").unwrap();
        assert_eq!(id.sequence, 2);
        assert_eq!(id.name, "atest_unit");
        assert_eq!(id.parallel_group(), Some("a".to_string()));
    }

    #[test]
    fn parse_stage_id_rejects_invalid() {
        assert!(StageId::parse("nope").is_err());
        assert!(StageId::parse("_leading").is_err());
        assert!(StageId::parse("00_").is_err());
    }

    #[test]
    fn stage_id_ordering() {
        let a = StageId::parse("01_alpha").unwrap();
        let b = StageId::parse("02_beta").unwrap();
        let c = StageId::parse("02a_gamma").unwrap();
        let d = StageId::parse("02b_delta").unwrap();

        let mut stages = vec![d.clone(), b.clone(), c.clone(), a.clone()];
        stages.sort();
        assert_eq!(stages, vec![a, b, c, d]);
    }

    #[test]
    fn stage_id_display() {
        let id = StageId::parse("01_analyze").unwrap();
        assert_eq!(format!("{id}"), "01_analyze");
    }
}
