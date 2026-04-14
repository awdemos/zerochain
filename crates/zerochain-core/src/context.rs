use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct ContextFrontmatter {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub human_gate: bool,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub definition_of_done: Option<String>,
}

impl Default for ContextFrontmatter {
    fn default() -> Self {
        Self {
            role: None,
            container: None,
            command: None,
            human_gate: false,
            timeout: None,
            network: None,
            definition_of_done: None,
        }
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Context {
    pub frontmatter: ContextFrontmatter,
    pub body: String,
    pub source_path: Option<std::path::PathBuf>,
}

impl Context {
    pub async fn from_file(path: &Path) -> Result<Self> {
        let content = tokio::fs::read_to_string(path).await.map_err(|e| Error::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let mut ctx = Self::parse(&content)?;
        ctx.source_path = Some(path.to_path_buf());
        Ok(ctx)
    }

    pub fn parse(content: &str) -> Result<Self> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return Ok(Context {
                frontmatter: ContextFrontmatter::default(),
                body: content.to_string(),
                source_path: None,
            });
        }

        let after_first = &trimmed[3..];

        let end_marker = after_first.find("\n---").ok_or_else(|| Error::YamlParse {
            path: std::path::PathBuf::from("<inline>"),
            source: serde_yaml::from_str::<serde_yaml::Value>("---").unwrap_err(),
        })?;

        let yaml_str = &after_first[..end_marker];
        let body = after_first[end_marker + 4..].trim_start().to_string();

        let frontmatter: ContextFrontmatter =
            serde_yaml::from_str(yaml_str).map_err(|e| Error::YamlParse {
                path: std::path::PathBuf::from("<inline>"),
                source: e,
            })?;

        Ok(Context {
            frontmatter,
            body,
            source_path: None,
        })
    }

    pub fn flatten(&self, parent: Option<&Context>) -> Context {
        let base = match parent {
            Some(p) => p.frontmatter.clone(),
            None => ContextFrontmatter::default(),
        };

        let merged = ContextFrontmatter {
            role: self.frontmatter.role.clone().or(base.role),
            container: self.frontmatter.container.clone().or(base.container),
            command: self.frontmatter.command.clone().or(base.command),
            human_gate: self.frontmatter.human_gate || base.human_gate,
            timeout: self.frontmatter.timeout.or(base.timeout),
            network: self.frontmatter.network.clone().or(base.network),
            definition_of_done: self
                .frontmatter
                .definition_of_done
                .clone()
                .or(base.definition_of_done),
        };

        Context {
            frontmatter: merged,
            body: self.body.clone(),
            source_path: self.source_path.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_frontmatter() {
        let input = r#"---
role: analyst
container: python:3.12
command: python analyze.py
human_gate: true
timeout: 300
network: host
definition_of_done: "All files processed"
---
# Analysis Stage
Do the analysis here.
"#;
        let ctx = Context::parse(input).unwrap();
        assert_eq!(ctx.frontmatter.role.as_deref(), Some("analyst"));
        assert_eq!(ctx.frontmatter.container.as_deref(), Some("python:3.12"));
        assert_eq!(
            ctx.frontmatter.command.as_deref(),
            Some("python analyze.py")
        );
        assert!(ctx.frontmatter.human_gate);
        assert_eq!(ctx.frontmatter.timeout, Some(300));
        assert_eq!(ctx.frontmatter.network.as_deref(), Some("host"));
        assert!(ctx.body.starts_with("# Analysis Stage"));
    }

    #[test]
    fn parse_no_frontmatter() {
        let input = "# Just markdown\nNo frontmatter here.\n";
        let ctx = Context::parse(input).unwrap();
        assert!(ctx.frontmatter.role.is_none());
        assert!(!ctx.frontmatter.human_gate);
        assert!(ctx.body.contains("No frontmatter here"));
    }

    #[test]
    fn parse_empty_frontmatter() {
        let input = "---\n---\nBody only";
        let ctx = Context::parse(input).unwrap();
        assert!(ctx.frontmatter.role.is_none());
        assert_eq!(ctx.body, "Body only");
    }

    #[test]
    fn flatten_inherits_from_parent() {
        let parent_input = "---\nrole: default_role\ntimeout: 60\n---\nParent body";
        let child_input = "---\ncommand: child_cmd\n---\nChild body";

        let parent = Context::parse(parent_input).unwrap();
        let child = Context::parse(child_input).unwrap();
        let merged = child.flatten(Some(&parent));

        assert_eq!(merged.frontmatter.role.as_deref(), Some("default_role"));
        assert_eq!(merged.frontmatter.command.as_deref(), Some("child_cmd"));
        assert_eq!(merged.frontmatter.timeout, Some(60));
        assert_eq!(merged.body, "Child body");
    }

    #[test]
    fn flatten_child_overrides_parent() {
        let parent_input = "---\nrole: parent\ncommand: parent_cmd\ntimeout: 30\n---\n";
        let child_input = "---\nrole: child\n---\n";

        let parent = Context::parse(parent_input).unwrap();
        let child = Context::parse(child_input).unwrap();
        let merged = child.flatten(Some(&parent));

        assert_eq!(merged.frontmatter.role.as_deref(), Some("child"));
        assert_eq!(merged.frontmatter.command.as_deref(), Some("parent_cmd"));
        assert_eq!(merged.frontmatter.timeout, Some(30));
    }
}
