use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Actor string identifying who or what performed an action.
///
/// OKF uses `<producer>/<version>` for agents/tools, `human:<id>` for people,
/// and `process:<id>` for automated processes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct OkfActor {
    pub by: String,
    #[serde(with = "chrono::serde::ts_seconds_option", default)]
    pub at: Option<DateTime<Utc>>,
}

impl OkfActor {
    #[must_use]
    pub fn new(by: impl Into<String>) -> Self {
        Self {
            by: by.into(),
            at: Some(Utc::now()),
        }
    }
}

/// A single source material a concept derives from.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct OkfSource {
    pub id: Option<String>,
    pub resource: String,
    pub title: Option<String>,
    pub author: Option<String>,
}

/// OKF v0.2 frontmatter for a knowledge concept.
///
/// See <https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md>.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OkfFrontmatter {
    #[serde(rename = "type")]
    pub okf_type: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub resource: Option<String>,
    pub tags: Vec<String>,
    pub generated: Option<OkfActor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verified: Vec<OkfActor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<OkfSource>,
    pub status: Option<String>,
    pub stale_after: Option<NaiveDate>,
}

/// Returns the default zerochain actor string: `zerochain/<version>`.
///
/// The version is taken from the crate's `CARGO_PKG_VERSION`. The result can
/// be overridden by setting `ZEROCHAIN_OKF_ACTOR`.
#[must_use]
pub fn zerochain_actor() -> String {
    std::env::var("ZEROCHAIN_OKF_ACTOR")
        .unwrap_or_else(|_| format!("zerochain/{}", env!("CARGO_PKG_VERSION")))
}

/// Serialize an [`OkfFrontmatter`] and body into an OKF concept document.
///
/// # Errors
///
/// Returns an error if the frontmatter cannot be serialized to YAML.
pub fn to_md_with_frontmatter(frontmatter: &OkfFrontmatter, body: &str) -> Result<String> {
    let yaml = serde_yml::to_string(frontmatter).map_err(|e| Error::PlanError {
        reason: format!("failed to serialize OKF frontmatter: {e}"),
    })?;
    Ok(format!("---\n{yaml}---\n\n{body}"))
}

/// Parse only the OKF frontmatter from a markdown concept document.
///
/// Returns a default [`OkfFrontmatter`] if no frontmatter block is present.
///
/// # Errors
///
/// Returns an error if a frontmatter block exists but is not valid YAML.
pub fn parse_frontmatter_only(content: &str) -> Result<OkfFrontmatter> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Ok(OkfFrontmatter::default());
    }

    let after_first = &trimmed[3..];
    let end_marker = after_first.find("\n---").ok_or_else(|| Error::YamlParse {
        path: std::path::PathBuf::from("<inline>"),
        source: serde_yml::from_str::<serde_yml::Value>("---").unwrap_err(),
    })?;

    let yaml_str = &after_first[..end_marker];
    serde_yml::from_str(yaml_str).map_err(|e| Error::YamlParse {
        path: std::path::PathBuf::from("<inline>"),
        source: e,
    })
}

/// Split an OKF concept document into its frontmatter and body.
///
/// If no frontmatter block is present, the returned frontmatter is [`OkfFrontmatter::default()`]
/// and the body is the original content.
///
/// # Errors
///
/// Returns an error if a frontmatter block exists but is not valid YAML.
pub fn split_frontmatter(content: &str) -> Result<(OkfFrontmatter, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Ok((OkfFrontmatter::default(), content.to_string()));
    }

    let after_first = &trimmed[3..];
    let end_marker = after_first.find("\n---").ok_or_else(|| Error::YamlParse {
        path: std::path::PathBuf::from("<inline>"),
        source: serde_yml::from_str::<serde_yml::Value>("---").unwrap_err(),
    })?;

    let yaml_str = &after_first[..end_marker];
    let body = after_first[end_marker + 4..].trim_start().to_string();
    let frontmatter: OkfFrontmatter =
        serde_yml::from_str(yaml_str).map_err(|e| Error::YamlParse {
            path: std::path::PathBuf::from("<inline>"),
            source: e,
        })?;

    Ok((frontmatter, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zerochain_actor_default_uses_version() {
        let actor = zerochain_actor();
        assert!(
            actor.starts_with("zerochain/"),
            "actor should start with zerochain/: {actor}"
        );
        assert!(
            actor.len() > "zerochain/".len(),
            "actor should include a version: {actor}"
        );
    }

    #[test]
    fn zerochain_actor_env_override() {
        let prev = std::env::var("ZEROCHAIN_OKF_ACTOR").ok();
        std::env::set_var("ZEROCHAIN_OKF_ACTOR", "custom/actor");
        assert_eq!(zerochain_actor(), "custom/actor");
        match prev {
            Some(v) => std::env::set_var("ZEROCHAIN_OKF_ACTOR", v),
            None => std::env::remove_var("ZEROCHAIN_OKF_ACTOR"),
        }
    }

    #[test]
    fn to_md_with_frontmatter_round_trips() {
        let fm = OkfFrontmatter {
            okf_type: "Stage Output".into(),
            title: Some("01_plan".into()),
            description: Some("Plan the work".into()),
            generated: Some(OkfActor::new("zerochain/0.1.0")),
            status: Some("stable".into()),
            ..Default::default()
        };
        let body = "# Result\n\nDone.";
        let doc = to_md_with_frontmatter(&fm, body).unwrap();
        assert!(doc.starts_with("---\n"));
        assert!(doc.contains("type: Stage Output"));
        assert!(doc.contains("# Result"));

        let (parsed_fm, parsed_body) = split_frontmatter(&doc).unwrap();
        assert_eq!(parsed_fm.okf_type, "Stage Output");
        assert_eq!(parsed_body, body);
    }

    #[test]
    fn parse_frontmatter_only_defaults_without_frontmatter() {
        let fm = parse_frontmatter_only("just body").unwrap();
        assert_eq!(fm.okf_type, String::default());
        assert!(fm.title.is_none());
    }

    #[test]
    fn verified_round_trips_as_list_or_single() {
        let fm = OkfFrontmatter {
            okf_type: "Metric".into(),
            verified: vec![
                OkfActor::new("human:alice"),
                OkfActor::new("process:nightly"),
            ],
            ..Default::default()
        };
        let yaml = serde_yml::to_string(&fm).unwrap();
        assert!(yaml.contains("human:alice"));

        let parsed: OkfFrontmatter = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(parsed.verified.len(), 2);
    }
}
