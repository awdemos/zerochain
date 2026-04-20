use std::path::Path;

use crate::error::{io_err, Error, Result};

// Re-export for backward compatibility with consumers that import via context.
pub use crate::frontmatter::{ContextFrontmatter, MultimodalInput};

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Context {
    pub frontmatter: ContextFrontmatter,
    pub body: String,
    pub source_path: Option<std::path::PathBuf>,
}

impl Context {
    /// Load a context from a Markdown file at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or if the Markdown frontmatter is invalid.
    pub async fn from_md_file(path: &Path) -> Result<Self> {
        let content = tokio::fs::read_to_string(path).await.map_err(|e| io_err(path.to_path_buf(), e))?;
        let mut ctx = Self::parse(&content)?;
        ctx.source_path = Some(path.to_path_buf());
        Ok(ctx)
    }

    /// Load a context from a Lua script at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or if the Lua script evaluation fails.
    pub async fn from_lua_file(path: &Path) -> Result<Self> {
        let content = tokio::fs::read_to_string(path).await.map_err(|e| io_err(path.to_path_buf(), e))?;
        let frontmatter = crate::lua_engine::eval_context_lua(&content)?;
        Ok(Context {
            frontmatter,
            body: String::new(),
            source_path: Some(path.to_path_buf()),
        })
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
            #[allow(clippy::unwrap_used)]
            source: serde_yml::from_str::<serde_yml::Value>("---").unwrap_err(),
        })?;

        let yaml_str = &after_first[..end_marker];
        let body = after_first[end_marker + 4..].trim_start().to_string();

        let frontmatter: ContextFrontmatter =
            serde_yml::from_str(yaml_str).map_err(|e| Error::YamlParse {
                path: std::path::PathBuf::from("<inline>"),
                source: e,
            })?;

        Ok(Context {
            frontmatter,
            body,
            source_path: None,
        })
    }

    #[must_use] pub fn flatten(&self, parent: Option<&Context>) -> Context {
        let base = match parent {
            Some(p) => p.frontmatter.clone(),
            None => ContextFrontmatter::default(),
        };

        Context {
            frontmatter: self.frontmatter.merge(&base),
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

    #[test]
    fn parse_kimi_k2_profile_frontmatter() {
        let input = r"---
provider_profile: kimi-k2
role: senior code reviewer
thinking_mode: disabled
capture_reasoning: true
---
Review the code.
";
        let ctx = Context::parse(input).unwrap();
        assert_eq!(ctx.frontmatter.provider_profile.as_deref(), Some("kimi-k2"));
        assert_eq!(ctx.frontmatter.role.as_deref(), Some("senior code reviewer"));
        assert_eq!(ctx.frontmatter.thinking_mode.as_deref(), Some("disabled"));
        assert!(ctx.frontmatter.capture_reasoning);
    }

    #[test]
    fn parse_multimodal_input_frontmatter() {
        let input = r#"---
multimodal_input:
  - type: image
    path: "./wireframes/auth.png"
    detail: high
---
Check the wireframe.
"#;
        let ctx = Context::parse(input).unwrap();
        assert_eq!(ctx.frontmatter.multimodal_input.len(), 1);
        assert_eq!(ctx.frontmatter.multimodal_input[0].input_type, "image");
        assert_eq!(ctx.frontmatter.multimodal_input[0].path, "./wireframes/auth.png");
        assert_eq!(ctx.frontmatter.multimodal_input[0].detail.as_deref(), Some("high"));
    }

    #[test]
    fn parse_default_fields_are_none_or_empty() {
        let input = "---\nrole: test\n---\nBody";
        let ctx = Context::parse(input).unwrap();
        assert!(ctx.frontmatter.provider_profile.is_none());
        assert!(ctx.frontmatter.thinking_mode.is_none());
        assert!(!ctx.frontmatter.capture_reasoning);
        assert!(ctx.frontmatter.multimodal_input.is_empty());
    }

    #[test]
    fn flatten_inherits_provider_profile() {
        let parent = Context::parse("---\nprovider_profile: kimi-k2\n---\n").unwrap();
        let child = Context::parse("---\n---\nChild").unwrap();
        let merged = child.flatten(Some(&parent));
        assert_eq!(merged.frontmatter.provider_profile.as_deref(), Some("kimi-k2"));
    }

    #[test]
    fn flatten_child_overrides_provider_profile() {
        let parent = Context::parse("---\nprovider_profile: kimi-k2\n---\n").unwrap();
        let child = Context::parse("---\nprovider_profile: generic\n---\n").unwrap();
        let merged = child.flatten(Some(&parent));
        assert_eq!(merged.frontmatter.provider_profile.as_deref(), Some("generic"));
    }

    #[test]
    fn flatten_inherits_thinking_mode() {
        let parent = Context::parse("---\nthinking_mode: extended\n---\n").unwrap();
        let child = Context::parse("---\n---\n").unwrap();
        let merged = child.flatten(Some(&parent));
        assert_eq!(merged.frontmatter.thinking_mode.as_deref(), Some("extended"));
    }

    #[test]
    fn flatten_capture_reasoning_or_logic() {
        let parent = Context::parse("---\ncapture_reasoning: true\n---\n").unwrap();
        let child = Context::parse("---\n---\n").unwrap();
        let merged = child.flatten(Some(&parent));
        assert!(merged.frontmatter.capture_reasoning);

        let parent2 = Context::parse("---\n---\n").unwrap();
        let child2 = Context::parse("---\ncapture_reasoning: true\n---\n").unwrap();
        let merged2 = child2.flatten(Some(&parent2));
        assert!(merged2.frontmatter.capture_reasoning);
    }

    #[test]
    fn flatten_multimodal_input_child_takes_precedence() {
        let parent = Context::parse(r"---
multimodal_input:
  - type: image
    path: parent.png
---
").unwrap();
        let child = Context::parse(r"---
multimodal_input:
  - type: image
    path: child.png
    detail: low
---
").unwrap();
        let merged = child.flatten(Some(&parent));
        assert_eq!(merged.frontmatter.multimodal_input.len(), 1);
        assert_eq!(merged.frontmatter.multimodal_input[0].path, "child.png");
    }

    #[test]
    fn flatten_multimodal_input_inherits_when_child_empty() {
        let parent = Context::parse(r"---
multimodal_input:
  - type: image
    path: parent.png
---
").unwrap();
        let child = Context::parse("---\n---\n").unwrap();
        let merged = child.flatten(Some(&parent));
        assert_eq!(merged.frontmatter.multimodal_input.len(), 1);
        assert_eq!(merged.frontmatter.multimodal_input[0].path, "parent.png");
    }
}
