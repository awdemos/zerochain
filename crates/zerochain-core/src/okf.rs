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
    #[serde(default)]
    pub tags: Vec<String>,
    pub generated: Option<OkfActor>,
    #[serde(
        default,
        deserialize_with = "deserialize_verified",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub verified: Vec<OkfActor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<OkfSource>,
    pub status: Option<String>,
    pub stale_after: Option<NaiveDate>,
}

/// `verified` accepts either a sequence of actors or a bare single actor
/// mapping (hand-edited concept files often use the short form).
#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrManyActors {
    One(OkfActor),
    Many(Vec<OkfActor>),
}

fn deserialize_verified<'de, D>(deserializer: D) -> std::result::Result<Vec<OkfActor>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match OneOrManyActors::deserialize(deserializer)? {
        OneOrManyActors::One(one) => Ok(vec![one]),
        OneOrManyActors::Many(many) => Ok(many),
    }
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

/// Locate the closing `---` delimiter line in the text that follows the
/// opening `---` line.
///
/// Only a line that is exactly `---` terminates the frontmatter block: a
/// `---junk` line or a `---` line inside a multi-line quoted YAML scalar must
/// not match. Both LF and CRLF line endings are accepted, as is a trailing
/// `\n---` at the very end of input.
///
/// Returns `(yaml_end, body_start)` byte offsets into `after_first`.
pub(crate) fn find_closing_delimiter(after_first: &str) -> Option<(usize, usize)> {
    let bytes = after_first.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\n' && bytes[i + 1..].starts_with(b"---") {
            let after = i + 4;
            let closes = after >= bytes.len()
                || bytes[after] == b'\n'
                || (bytes[after] == b'\r' && bytes.get(after + 1) == Some(&b'\n'));
            if closes {
                let body_start = if after >= bytes.len() {
                    after
                } else if bytes[after] == b'\r' {
                    after + 2
                } else {
                    after + 1
                };
                return Some((i, body_start));
            }
        }
        i += 1;
    }
    None
}

/// Returns the input with a single leading UTF-8 BOM stripped, if present.
pub(crate) fn strip_bom(content: &str) -> &str {
    content.strip_prefix('\u{FEFF}').unwrap_or(content)
}

/// Detects a frontmatter block: the first non-blank line must be exactly
/// `---` (a `----` CommonMark thematic break does not count).
pub(crate) fn starts_frontmatter(trimmed: &str) -> bool {
    let first_line = trimmed.split('\n').next().unwrap_or(trimmed);
    first_line.trim_end_matches('\r') == "---"
}

/// Parse only the OKF frontmatter from a markdown concept document.
///
/// Returns a default [`OkfFrontmatter`] if no frontmatter block is present.
///
/// # Errors
///
/// Returns an error if a frontmatter block exists but is not valid YAML.
pub fn parse_frontmatter_only(content: &str) -> Result<OkfFrontmatter> {
    let trimmed = strip_bom(content).trim_start();
    if !starts_frontmatter(trimmed) {
        return Ok(OkfFrontmatter::default());
    }

    let after_first = &trimmed[3..];
    let (yaml_end, _) =
        find_closing_delimiter(after_first).ok_or_else(|| Error::UnterminatedFrontmatter {
            path: std::path::PathBuf::from("<inline>"),
        })?;

    let yaml_str = &after_first[..yaml_end];
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
    let trimmed = strip_bom(content).trim_start();
    if !starts_frontmatter(trimmed) {
        return Ok((OkfFrontmatter::default(), content.to_string()));
    }

    let after_first = &trimmed[3..];
    let (yaml_end, body_start) =
        find_closing_delimiter(after_first).ok_or_else(|| Error::UnterminatedFrontmatter {
            path: std::path::PathBuf::from("<inline>"),
        })?;

    let yaml_str = &after_first[..yaml_end];
    let body = after_first[body_start..].trim_start().to_string();
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

    #[test]
    fn unclosed_frontmatter_errors_instead_of_panicking() {
        let err = split_frontmatter("---\nrole: x\n").unwrap_err();
        assert!(
            matches!(err, Error::UnterminatedFrontmatter { .. }),
            "expected UnterminatedFrontmatter, got: {err:?}"
        );
        let err = parse_frontmatter_only("---\ntype: Metric\n").unwrap_err();
        assert!(matches!(err, Error::UnterminatedFrontmatter { .. }));
    }

    #[test]
    fn dashed_junk_line_is_not_a_closing_delimiter() {
        // `---junk` must not terminate the frontmatter; with no real closing
        // delimiter the parse must error rather than silently truncate.
        let md = "---\ntype: Metric\n---junk\nbody";
        assert!(split_frontmatter(md).is_err());
        assert!(parse_frontmatter_only(md).is_err());
    }

    #[test]
    fn four_dash_thematic_break_is_not_frontmatter() {
        // `----` is a CommonMark thematic break, not a frontmatter fence.
        let md = "----\nSome body\n";
        let (fm, body) = split_frontmatter(md).unwrap();
        assert_eq!(fm, OkfFrontmatter::default());
        assert_eq!(body, md);

        let fm = parse_frontmatter_only(md).unwrap();
        assert_eq!(fm, OkfFrontmatter::default());
    }

    #[test]
    fn dash_line_inside_multiline_scalar_does_not_close_frontmatter() {
        let md = "---\ntype: Metric\ndescription: |\n  line one\n  ---\n  line three\n---\nBody";
        let (fm, body) = split_frontmatter(md).unwrap();
        assert_eq!(fm.okf_type, "Metric");
        assert_eq!(body, "Body");
    }

    #[test]
    fn crlf_closing_delimiter_is_accepted() {
        let md = "---\r\ntype: Metric\r\n---\r\nBody\r\nmore";
        let (fm, body) = split_frontmatter(md).unwrap();
        assert_eq!(fm.okf_type, "Metric");
        assert_eq!(body, "Body\r\nmore");
    }

    #[test]
    fn trailing_closing_delimiter_at_eof_has_empty_body() {
        let md = "---\ntype: Metric\n---";
        let (fm, body) = split_frontmatter(md).unwrap();
        assert_eq!(fm.okf_type, "Metric");
        assert_eq!(body, "");
    }

    #[test]
    fn bom_prefixed_frontmatter_is_detected() {
        let md = "\u{FEFF}---\ntype: Metric\n---\nBody";
        let (fm, body) = split_frontmatter(md).unwrap();
        assert_eq!(fm.okf_type, "Metric");
        assert_eq!(body, "Body");

        let fm = parse_frontmatter_only(md).unwrap();
        assert_eq!(fm.okf_type, "Metric");
    }

    #[test]
    fn tags_default_to_empty_when_omitted() {
        let yaml = "type: Metric\nverified:\n  by: human:alice\n";
        let fm: OkfFrontmatter = serde_yml::from_str(yaml).unwrap();
        assert!(fm.tags.is_empty());
        assert_eq!(fm.verified.len(), 1);
        assert_eq!(fm.verified[0].by, "human:alice");

        let doc = format!("---\n{yaml}---\nBody");
        let (fm, _) = split_frontmatter(&doc).unwrap();
        assert!(fm.tags.is_empty());
        assert_eq!(fm.verified.len(), 1);
    }

    #[test]
    fn verified_accepts_bare_single_mapping() {
        let yaml = "type: Metric\nverified:\n  by: process:nightly\n";
        let fm: OkfFrontmatter = serde_yml::from_str(yaml).unwrap();
        assert_eq!(fm.verified.len(), 1);
        assert_eq!(fm.verified[0].by, "process:nightly");
    }
}
