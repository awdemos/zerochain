use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::MemoryError;
use crate::Result;

/// Reserved contribution types (spec §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContributionType {
    Setup,
    Result,
    Insight,
    Hypothesis,
    Verification,
    Report,
}

impl ContributionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContributionType::Setup => "setup",
            ContributionType::Result => "result",
            ContributionType::Insight => "insight",
            ContributionType::Hypothesis => "hypothesis",
            ContributionType::Verification => "verification",
            ContributionType::Report => "report",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "setup" => Ok(ContributionType::Setup),
            "result" => Ok(ContributionType::Result),
            "insight" => Ok(ContributionType::Insight),
            "hypothesis" => Ok(ContributionType::Hypothesis),
            "verification" => Ok(ContributionType::Verification),
            "report" => Ok(ContributionType::Report),
            other => Err(MemoryError::InvalidInput(format!(
                "unknown contribution type: {other}"
            ))),
        }
    }
}

/// Verification verdict (spec §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Confirmed,
    Partial,
    Failed,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Confirmed => "confirmed",
            Verdict::Partial => "partial",
            Verdict::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "confirmed" => Ok(Verdict::Confirmed),
            "partial" => Ok(Verdict::Partial),
            "failed" => Ok(Verdict::Failed),
            other => Err(MemoryError::InvalidInput(format!(
                "unknown verdict: {other}"
            ))),
        }
    }
}

/// Whether a lower or higher metric value is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricDirection {
    Lower,
    Higher,
}

impl MetricDirection {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "lower" => Ok(MetricDirection::Lower),
            "higher" => Ok(MetricDirection::Higher),
            other => Err(MemoryError::InvalidInput(format!(
                "unknown metric direction: {other}"
            ))),
        }
    }
}

/// Optional numeric metric attached to a contribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContributionMetric {
    pub name: String,
    pub value: f64,
    pub direction: MetricDirection,
}

/// A single immutable contribution to the shared graph.
#[derive(Debug, Clone, PartialEq)]
pub struct ContributionRecord {
    pub id: String,
    pub record_type: ContributionType,
    pub parents: Vec<String>,
    pub actor: String,
    pub created: DateTime<Utc>,
    pub workflow: Option<String>,
    pub stage: Option<String>,
    pub metric: Option<ContributionMetric>,
    pub tags: Vec<String>,
    pub artifacts: Vec<String>,
    pub verdict: Option<Verdict>,
    pub target: Option<String>,
    pub body: String,
}

/// Serialization shape used for the canonical content hash. `serde_json::Map`
/// is a `BTreeMap` (sorted keys), so this is canonical for a given record.
#[derive(Serialize)]
struct CanonicalRecord<'a> {
    #[serde(rename = "type")]
    record_type: ContributionType,
    parents: &'a [String],
    actor: &'a str,
    created: DateTime<Utc>,
    workflow: Option<&'a str>,
    stage: Option<&'a str>,
    metric: Option<&'a ContributionMetric>,
    tags: &'a [String],
    artifacts: &'a [String],
    verdict: Option<Verdict>,
    target: Option<&'a str>,
    body: &'a str,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RecordFrontmatter {
    #[serde(rename = "type")]
    record_type: Option<String>,
    #[serde(default)]
    parents: Vec<String>,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    created: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workflow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metric: Option<ContributionMetric>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifacts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}

impl ContributionRecord {
    pub fn new(record_type: ContributionType, actor: impl Into<String>, body: impl Into<String>) -> Self {
        ContributionRecord {
            id: String::new(),
            record_type,
            parents: Vec::new(),
            actor: actor.into(),
            created: Utc::now(),
            workflow: None,
            stage: None,
            metric: None,
            tags: Vec::new(),
            artifacts: Vec::new(),
            verdict: None,
            target: None,
            body: body.into(),
        }
    }

    /// Content-addressed ID: `c-` + first 16 hex chars of blake3 over the
    /// canonical JSON serialization. Publishing the identical record twice
    /// yields the identical ID (spec §3).
    pub fn compute_id(&self) -> String {
        let canonical = CanonicalRecord {
            record_type: self.record_type,
            parents: &self.parents,
            actor: &self.actor,
            created: self.created,
            workflow: self.workflow.as_deref(),
            stage: self.stage.as_deref(),
            metric: self.metric.as_ref(),
            tags: &self.tags,
            artifacts: &self.artifacts,
            verdict: self.verdict,
            target: self.target.as_deref(),
            body: &self.body,
        };
        let json = serde_json::to_string(&canonical)
            .unwrap_or_else(|e| MemoryError::Serialization(e.to_string()).to_string());
        let hash = blake3::hash(json.as_bytes()).to_hex();
        format!("c-{}", &hash[..16])
    }

    /// Structural validation, independent of store referential checks.
    pub fn validate(&self) -> Result<()> {
        if self.record_type == ContributionType::Hypothesis && self.metric.is_some() {
            return Err(MemoryError::InvalidInput(
                "hypothesis must not carry a metric (untested proposal)".to_string(),
            ));
        }
        if self.record_type == ContributionType::Verification {
            if self.target.is_none() {
                return Err(MemoryError::InvalidInput(
                    "verification requires a target".to_string(),
                ));
            }
            if self.verdict.is_none() {
                return Err(MemoryError::InvalidInput(
                    "verification requires a verdict".to_string(),
                ));
            }
        } else if self.target.is_some() || self.verdict.is_some() {
            return Err(MemoryError::InvalidInput(
                "target/verdict are only valid on verification records".to_string(),
            ));
        }
        Ok(())
    }

    pub fn to_markdown(&self) -> Result<String> {
        let fm = RecordFrontmatter {
            record_type: Some(self.record_type.as_str().to_string()),
            parents: self.parents.clone(),
            actor: Some(self.actor.clone()),
            created: Some(self.created),
            workflow: self.workflow.clone(),
            stage: self.stage.clone(),
            metric: self.metric.clone(),
            tags: self.tags.clone(),
            artifacts: self.artifacts.clone(),
            verdict: self.verdict.map(|v| v.as_str().to_string()),
            target: self.target.clone(),
        };
        let yaml = serde_yml::to_string(&fm).map_err(|e| MemoryError::Serialization(e.to_string()))?;
        Ok(format!("---\n{yaml}---\n\n{}", self.body))
    }

    pub fn from_markdown(content: &str) -> Result<Self> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return Err(MemoryError::InvalidInput(
                "contribution record missing frontmatter".to_string(),
            ));
        }
        let after_first = &trimmed[3..];
        let end_marker = after_first
            .find("\n---")
            .ok_or_else(|| MemoryError::InvalidInput("unclosed frontmatter".to_string()))?;
        let yaml_str = &after_first[..end_marker];
        let body = after_first[end_marker + 4..].trim_start().to_string();
        let fm: RecordFrontmatter = serde_yml::from_str(yaml_str)
            .map_err(|e| MemoryError::Serialization(e.to_string()))?;

        let record_type = ContributionType::parse(
            fm.record_type
                .as_deref()
                .ok_or_else(|| MemoryError::InvalidInput("missing type".to_string()))?,
        )?;
        let record = ContributionRecord {
            id: String::new(),
            record_type,
            parents: fm.parents,
            actor: fm.actor.unwrap_or_default(),
            created: fm.created.unwrap_or_else(Utc::now),
            workflow: fm.workflow,
            stage: fm.stage,
            metric: fm.metric,
            tags: fm.tags,
            artifacts: fm.artifacts,
            verdict: fm
                .verdict
                .map(|v| Verdict::parse(&v))
                .transpose()?,
            target: fm.target,
            body,
        };
        Ok(record)
    }

    /// Parse a record file and verify its embedded `id:` matches its content.
    pub fn from_markdown_with_id(content: &str, id: &str) -> Result<Self> {
        let mut record = Self::from_markdown(content)?;
        if record.compute_id() != id {
            return Err(MemoryError::InvalidInput(format!(
                "record id {id} does not match content hash"
            )));
        }
        record.id = id.to_string();
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> ContributionRecord {
        let mut rec = ContributionRecord::new(
            ContributionType::Result,
            "zerochain/0.2.0",
            "Six-donor blend with joint temperature retune.",
        );
        rec.parents = vec!["c-aaaaaaaaaaaaaaaa".to_string()];
        rec.workflow = Some("weight-transfer".to_string());
        rec.stage = Some("03_eval".to_string());
        rec.metric = Some(ContributionMetric {
            name: "bpb".to_string(),
            value: 1.905,
            direction: MetricDirection::Lower,
        });
        rec.tags = vec!["multi-donor".to_string()];
        rec.artifacts = vec!["b3:abc123".to_string()];
        rec
    }

    #[test]
    fn compute_id_is_deterministic_and_prefixed() {
        let rec = sample_record();
        let id1 = rec.compute_id();
        let id2 = rec.clone().compute_id();
        assert_eq!(id1, id2);
        assert!(id1.starts_with("c-"));
        assert_eq!(id1.len(), 2 + 16);
    }

    #[test]
    fn parent_order_changes_id() {
        let mut rec = sample_record();
        rec.parents = vec!["c-bbbbbbbbbbbbbbbb".to_string(), "c-aaaaaaaaaaaaaaaa".to_string()];
        assert_ne!(rec.compute_id(), sample_record().compute_id());
    }

    #[test]
    fn markdown_round_trip_preserves_fields() {
        let mut rec = sample_record();
        rec.id = rec.compute_id();
        let md = rec.to_markdown().unwrap();
        let parsed = ContributionRecord::from_markdown_with_id(&md, &rec.id).unwrap();
        assert_eq!(parsed.record_type, rec.record_type);
        assert_eq!(parsed.parents, rec.parents);
        assert_eq!(parsed.actor, rec.actor);
        assert_eq!(parsed.workflow, rec.workflow);
        assert_eq!(parsed.metric, rec.metric);
        assert_eq!(parsed.tags, rec.tags);
        assert_eq!(parsed.artifacts, rec.artifacts);
        assert_eq!(parsed.body, rec.body);
    }

    #[test]
    fn validate_rejects_hypothesis_with_metric() {
        let mut rec = ContributionRecord::new(ContributionType::Hypothesis, "a", "try X");
        rec.metric = Some(ContributionMetric {
            name: "bpb".to_string(),
            value: 1.9,
            direction: MetricDirection::Lower,
        });
        assert!(rec.validate().is_err());
    }

    #[test]
    fn validate_rejects_verification_without_target_or_verdict() {
        let mut rec = ContributionRecord::new(ContributionType::Verification, "a", "reproduced");
        assert!(rec.validate().is_err());
        rec.target = Some("c-abc".to_string());
        assert!(rec.validate().is_err());
        rec.verdict = Some(Verdict::Confirmed);
        assert!(rec.validate().is_ok());
    }

    #[test]
    fn validate_rejects_verdict_on_result() {
        let mut rec = ContributionRecord::new(ContributionType::Result, "a", "ran it");
        rec.verdict = Some(Verdict::Confirmed);
        assert!(rec.validate().is_err());
    }

    #[test]
    fn type_and_verdict_parse_round_trip() {
        assert_eq!(ContributionType::parse("insight").unwrap(), ContributionType::Insight);
        assert!(ContributionType::parse("nope").is_err());
        assert_eq!(Verdict::parse("partial").unwrap(), Verdict::Partial);
        assert!(Verdict::parse("nope").is_err());
    }
}
