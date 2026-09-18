# Collective Contribution Graph Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a workspace-level, append-only contribution DAG (typed, content-addressed records with parent lineage) to zerochain, per the approved spec at `docs/superpowers/specs/2026-09-18-collective-contribution-graph-design.md`.

**Architecture:** Canonical state = one markdown file per contribution under `{workspace}/.zerochain/graph/contributions/`, derived index rebuilt in memory from files. `zerochain-memory` grows the graph layer (`ContributionRecord`, `ContributionStore`, `GraphIndex`, `Graph`, `GraphEmbedIndex`); `zerochain-engine` auto-captures `setup`/`result` nodes and injects graph context into LLM tools; `zerochain-tools` gains `contribute`/`verify`/`graph_query`; CLI + zerochaind expose human surfaces. Embeddings stay a derived cache. Legacy per-workflow `memory.jsonl` is untouched.

**Tech Stack:** Rust nightly workspace, tokio, serde/serde_yml, blake3, chrono, axum, clap, fastembed (existing deps — no new external crates; only new workspace-dep usages).

**Spec reference:** all section numbers below (`spec §N`) refer to the design spec.

---

## File structure

**Create:**
- `crates/zerochain-memory/src/record.rs` — contribution types, record, canonical hash, markdown round-trip, validation
- `crates/zerochain-memory/src/graph_store.rs` — `ContributionStore` (canonical files, publish/get/list)
- `crates/zerochain-memory/src/graph_index.rs` — `GraphIndex` (derived views) + `GraphView`
- `crates/zerochain-memory/src/graph.rs` — `Graph` (store + index, open/publish)
- `crates/zerochain-memory/src/graph_embed.rs` — `GraphEmbedIndex` (derived semantic search)
- `crates/zerochain-tools/src/graph_tool.rs` — `ContributeTool`, `VerifyTool`, `GraphQueryTool`
- `crates/zerochain-engine/tests/graph_tools.rs` — tool-loop integration test
- `crates/zerochain-daemon/tests/cli_graph_parse.rs` — clap parse tests
- `crates/zerochain-server/src/handlers/graph.rs` — HTTP handlers

**Modify:**
- `crates/zerochain-memory/Cargo.toml` (add blake3, chrono, serde_yml), `crates/zerochain-memory/src/lib.rs`
- `crates/zerochain-core/src/frontmatter.rs` (add `StageMetric`/`MetricDirection` + `metric` field), `crates/zerochain-core/src/task.rs` (add `parents`)
- `crates/zerochain-engine/src/state.rs` (graph field, setup node, `InitWorkflowParams.parents`), `crates/zerochain-engine/src/actor.rs`, `crates/zerochain-engine/src/registry.rs`, `crates/zerochain-engine/src/llm_driver.rs` (result auto-capture), `crates/zerochain-engine/src/tool_driver.rs` (injection)
- `crates/zerochain-tools/src/registry.rs`, `crates/zerochain-tools/src/lib.rs`
- `crates/zerochain-daemon/src/cli.rs`, `crates/zerochain-daemon/src/main.rs`, `crates/zerochain-daemon/src/mcp.rs`, `crates/zerochain-daemon/Cargo.toml` (add zerochain-memory)
- `crates/zerochain-server/src/state.rs`, `crates/zerochain-server/src/handlers/mod.rs`, `crates/zerochain-server/src/handlers/workflow.rs`, `crates/zerochain-server/Cargo.toml` (add zerochain-memory, tokio sync already via axum? add `tokio` workspace), `crates/zerochain-server/tests/integration.rs`
- `README.md`

**Conventions to follow:** unit tests colocated in `#[cfg(test)] mod tests` with `tempfile::TempDir`; `#[tokio::test]`; fake `EmbeddingModel` returning fixed vectors (never download weights in unit tests — the one exception is tool tests, which follow the existing `memory_tool.rs` precedent of using the real `FastEmbedModel`); atomic writes via `tmp` + rename; commits per task.

---

### Task 1: Contribution record type, canonical hash, markdown round-trip

**Files:**
- Modify: `crates/zerochain-memory/Cargo.toml`
- Create: `crates/zerochain-memory/src/record.rs`
- Modify: `crates/zerochain-memory/src/lib.rs`

- [ ] **Step 1: Add dependencies**

In `crates/zerochain-memory/Cargo.toml`, add to `[dependencies]`:

```toml
blake3.workspace = true
chrono.workspace = true
serde_yml.workspace = true
```

- [ ] **Step 2: Write the failing tests**

Create `crates/zerochain-memory/src/record.rs`:

```rust
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
```

- [ ] **Step 3: Register module and run tests**

In `crates/zerochain-memory/src/lib.rs` add `pub mod record;` after `pub mod model;` and re-export:

```rust
pub use record::{
    ContributionMetric, ContributionRecord, ContributionType, MetricDirection, Verdict,
};
```

Run: `cargo test -p zerochain-memory record 2>&1 | tail -20`
Expected: all 7 tests PASS on the first run (hash determinism, round-trip, and all validation rules are fully specified above). If any fail, fix the implementation — do not weaken the test.

- [ ] **Step 4: Commit**

```bash
git add crates/zerochain-memory/Cargo.toml crates/zerochain-memory/src/record.rs crates/zerochain-memory/src/lib.rs
git commit -m "feat(memory): add typed contribution record with content-hash IDs"
```

---

### Task 2: ContributionStore — canonical append-only file store

**Files:**
- Create: `crates/zerochain-memory/src/graph_store.rs`
- Modify: `crates/zerochain-memory/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/zerochain-memory/src/graph_store.rs`:

```rust
use std::path::{Path, PathBuf};

use crate::error::{io_err, MemoryError};
use crate::record::{ContributionRecord, ContributionType};
use crate::Result;

/// Canonical append-only store of contribution records, one markdown file per
/// record under `contributions/`. Files are never mutated after publish
/// (spec §4.2).
#[derive(Debug, Clone)]
pub struct ContributionStore {
    dir: PathBuf,
}

impl ContributionStore {
    pub async fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| io_err(&dir, e))?;
        Ok(ContributionStore { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Validate, content-address, and atomically write a record. Re-publishing
    /// the identical record is a success no-op (spec §4.2).
    pub async fn publish(&self, mut record: ContributionRecord) -> Result<ContributionRecord> {
        record.validate()?;
        if record.id.is_empty() {
            record.id = record.compute_id();
        }
        for parent in &record.parents {
            if self.get(parent).await?.is_none() {
                return Err(MemoryError::InvalidInput(format!(
                    "parent not found: {parent}"
                )));
            }
        }
        if let Some(target) = &record.target {
            match self.get(target).await? {
                None => {
                    return Err(MemoryError::InvalidInput(format!(
                        "verification target not found: {target}"
                    )));
                }
                Some(existing) if existing.record_type == ContributionType::Verification => {
                    return Err(MemoryError::InvalidInput(
                        "cannot verify a verification record".to_string(),
                    ));
                }
                _ => {}
            }
        }

        let content = record.to_markdown()?;
        let path = self.dir.join(format!("{}.md", record.id));
        match tokio::fs::read_to_string(&path).await {
            Ok(existing) if existing == content => return Ok(record),
            Ok(_) => {
                return Err(MemoryError::Other(format!(
                    "record id collision with different content: {}",
                    record.id
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(&path, e)),
        }
        let tmp_path = path.with_extension("md.tmp");
        tokio::fs::write(&tmp_path, &content)
            .await
            .map_err(|e| io_err(&tmp_path, e))?;
        tokio::fs::rename(&tmp_path, &path)
            .await
            .map_err(|e| io_err(&path, e))?;
        Ok(record)
    }

    pub async fn get(&self, id: &str) -> Result<Option<ContributionRecord>> {
        let path = self.dir.join(format!("{id}.md"));
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err(&path, e)),
        };
        Ok(Some(ContributionRecord::from_markdown_with_id(
            &content, id,
        )?))
    }

    /// All readable records. Corrupt files are skipped with a warning
    /// (spec §7).
    pub async fn list(&self) -> Result<Vec<ContributionRecord>> {
        let mut records = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.dir)
            .await
            .map_err(|e| io_err(&self.dir, e))?;
        while let Some(entry) = entries.next_entry().await.map_err(|e| io_err(&self.dir, e))? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
            else {
                continue;
            };
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => match ContributionRecord::from_markdown_with_id(&content, &id) {
                    Ok(record) => records.push(record),
                    Err(e) => tracing::warn!(path = %path.display(), error = %e, "skipping corrupt contribution record"),
                },
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "skipping unreadable contribution record"),
            }
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Verdict};
    use tempfile::TempDir;

    #[tokio::test]
    async fn publish_assigns_id_and_round_trips() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path()).await.unwrap();
        let rec = ContributionRecord::new(ContributionType::Setup, "zerochain/0.2.0", "brief");
        let published = store.publish(rec).await.unwrap();
        assert!(published.id.starts_with("c-"));
        let loaded = store.get(&published.id).await.unwrap().unwrap();
        assert_eq!(loaded.body, "brief");
        assert_eq!(loaded.record_type, ContributionType::Setup);
    }

    #[tokio::test]
    async fn republish_identical_record_is_noop() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path()).await.unwrap();
        let rec = ContributionRecord::new(ContributionType::Insight, "a", "same");
        let first = store.publish(rec.clone()).await.unwrap();
        let second = store.publish(rec).await.unwrap();
        assert_eq!(first.id, second.id);
        let mut entries = tokio::fs::read_dir(tmp.path()).await.unwrap();
        let mut count = 0;
        while entries.next_entry().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 1, "identical re-publish must not write a second file");
    }

    #[tokio::test]
    async fn publish_rejects_missing_parent() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path()).await.unwrap();
        let mut rec = ContributionRecord::new(ContributionType::Result, "a", "orphan");
        rec.parents = vec!["c-doesnotexist0000".to_string()];
        assert!(store.publish(rec).await.is_err());
    }

    #[tokio::test]
    async fn publish_chains_parents() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path()).await.unwrap();
        let setup = store
            .publish(ContributionRecord::new(ContributionType::Setup, "a", "root"))
            .await
            .unwrap();
        let mut child = ContributionRecord::new(ContributionType::Result, "a", "builds on root");
        child.parents = vec![setup.id.clone()];
        let child = store.publish(child).await.unwrap();
        assert_eq!(child.parents, vec![setup.id]);
    }

    #[tokio::test]
    async fn verification_of_verification_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path()).await.unwrap();
        let result = store
            .publish(ContributionRecord::new(ContributionType::Result, "a", "measured"))
            .await
            .unwrap();
        let mut verification = ContributionRecord::new(ContributionType::Verification, "b", "reproduced");
        verification.target = Some(result.id.clone());
        verification.verdict = Some(Verdict::Confirmed);
        verification.parents = vec![result.id.clone()];
        let verification = store.publish(verification).await.unwrap();
        let mut reverify = ContributionRecord::new(ContributionType::Verification, "c", "again");
        reverify.target = Some(verification.id.clone());
        reverify.verdict = Some(Verdict::Confirmed);
        reverify.parents = vec![verification.id.clone()];
        assert!(store.publish(reverify).await.is_err());
    }

    #[tokio::test]
    async fn list_skips_corrupt_files() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path()).await.unwrap();
        store
            .publish(ContributionRecord::new(ContributionType::Insight, "a", "good"))
            .await
            .unwrap();
        tokio::fs::write(tmp.path().join("c-corrupt0000000.md"), "not markdown at all")
            .await
            .unwrap();
        let records = store.list().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].body, "good");
    }
}
```

- [ ] **Step 2: Register module and run tests to verify failure**

In `crates/zerochain-memory/src/lib.rs`: add `pub mod graph_store;` and `pub use graph_store::ContributionStore;`.

Run: `cargo test -p zerochain-memory graph_store 2>&1 | tail -5`
Expected: compile error first (module missing) → after adding, all 6 tests PASS. If tests pass immediately, still verify each ran.

- [ ] **Step 3: Commit**

```bash
git add crates/zerochain-memory/src/graph_store.rs crates/zerochain-memory/src/lib.rs
git commit -m "feat(memory): add append-only contribution store with referential validation"
```

---

### Task 3: GraphIndex — derived views and verification supersession

**Files:**
- Create: `crates/zerochain-memory/src/graph_index.rs`
- Modify: `crates/zerochain-memory/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/zerochain-memory/src/graph_index.rs`:

```rust
use std::collections::{BTreeMap, HashMap};

use crate::record::{ContributionRecord, ContributionType, Verdict};

/// Named views over the graph (spec §4.2, §6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphView {
    Recent,
    Leaves,
    OpenHypotheses,
    Unverified,
    Negative,
    Leaders,
}

impl GraphView {
    pub fn as_str(&self) -> &'static str {
        match self {
            GraphView::Recent => "recent",
            GraphView::Leaves => "leaves",
            GraphView::OpenHypotheses => "open_hypotheses",
            GraphView::Unverified => "unverified",
            GraphView::Negative => "negative",
            GraphView::Leaders => "leaders",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "recent" => Some(GraphView::Recent),
            "leaves" => Some(GraphView::Leaves),
            "open_hypotheses" => Some(GraphView::OpenHypotheses),
            "unverified" => Some(GraphView::Unverified),
            "negative" => Some(GraphView::Negative),
            "leaders" => Some(GraphView::Leaders),
            _ => None,
        }
    }
}

/// Derived, rebuildable index over contribution records.
#[derive(Debug, Default)]
pub struct GraphIndex {
    records: BTreeMap<String, ContributionRecord>,
    children: HashMap<String, Vec<String>>,
}

impl GraphIndex {
    pub fn from_records(records: Vec<ContributionRecord>) -> Self {
        let mut index = GraphIndex::default();
        for record in records {
            index.add(record);
        }
        index
    }

    pub fn add(&mut self, record: ContributionRecord) {
        for parent in &record.parents {
            self.children
                .entry(parent.clone())
                .or_default()
                .push(record.id.clone());
        }
        self.records.insert(record.id.clone(), record);
    }

    pub fn get(&self, id: &str) -> Option<&ContributionRecord> {
        self.records.get(id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// All records, oldest first.
    pub fn all(&self) -> Vec<&ContributionRecord> {
        let mut recs: Vec<&ContributionRecord> = self.records.values().collect();
        recs.sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
        recs
    }

    /// Most recent contribution published in the given workflow — the parent
    /// chaining rule for auto-captured `result` nodes (spec §5).
    pub fn latest_in_workflow(&self, workflow: &str) -> Option<&ContributionRecord> {
        self.records
            .values()
            .filter(|r| r.workflow.as_deref() == Some(workflow))
            .max_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)))
    }

    /// Effective verdicts for a target: newest verification per actor.
    /// Replaceable-verdict semantics (spec §3): both records stay in the DAG;
    /// only the effect is superseded.
    pub fn effective_verdicts(&self, target: &str) -> Vec<(String, Verdict)> {
        let mut per_actor: HashMap<String, (chrono::DateTime<chrono::Utc>, Verdict)> =
            HashMap::new();
        for record in self.records.values() {
            if record.record_type != ContributionType::Verification {
                continue;
            }
            if record.target.as_deref() != Some(target) {
                continue;
            }
            let Some(actor) = (!record.actor.is_empty()).then(|| record.actor.clone()) else {
                continue;
            };
            let verdict = record.verdict.unwrap_or(Verdict::Failed);
            match per_actor.get(&actor) {
                Some((created, _)) if *created >= record.created => {}
                _ => {
                    per_actor.insert(actor, (record.created, verdict));
                }
            }
        }
        per_actor.into_iter().map(|(a, (_, v))| (a, v)).collect()
    }

    fn is_verified(&self, record: &ContributionRecord) -> bool {
        self.effective_verdicts(&record.id)
            .iter()
            .any(|(_, v)| matches!(v, Verdict::Confirmed | Verdict::Partial))
    }

    /// Records matching a named view.
    pub fn view(&self, view: GraphView) -> Vec<&ContributionRecord> {
        match view {
            GraphView::Recent => {
                let mut recs = self.all();
                recs.reverse();
                recs
            }
            // Frontier: childless records that can still be built on.
            // Verifications are never parents for new work, so exclude them.
            GraphView::Leaves => self
                .records
                .values()
                .filter(|r| {
                    r.record_type != ContributionType::Verification
                        && !self.children.contains_key(&r.id)
                })
                .collect(),
            GraphView::OpenHypotheses => self
                .records
                .values()
                .filter(|r| r.record_type == ContributionType::Hypothesis)
                .collect(),
            GraphView::Negative => self
                .records
                .values()
                .filter(|r| r.tags.iter().any(|t| t == "negative"))
                .collect(),
            // Results with no effective confirmed/partial verification from
            // any actor (spec §4.2).
            GraphView::Unverified => self
                .records
                .values()
                .filter(|r| r.record_type == ContributionType::Result && !self.is_verified(r))
                .collect(),
            // Best results per metric group, respecting direction.
            GraphView::Leaders => {
                let mut groups: HashMap<(String, crate::record::MetricDirection), Vec<&ContributionRecord>> =
                    HashMap::new();
                for record in self.records.values() {
                    if record.record_type != ContributionType::Result {
                        continue;
                    }
                    let Some(metric) = &record.metric else { continue };
                    groups
                        .entry((metric.name.clone(), metric.direction))
                        .or_default()
                        .push(record);
                }
                let mut leaders = Vec::new();
                for ((_, direction), mut group) in groups {
                    group.sort_by(|a, b| {
                        let ord = a
                            .metric
                            .as_ref()
                            .unwrap()
                            .value
                            .partial_cmp(&b.metric.as_ref().unwrap().value)
                            .unwrap_or(std::cmp::Ordering::Equal);
                        match direction {
                            crate::record::MetricDirection::Lower => ord,
                            crate::record::MetricDirection::Higher => ord.reverse(),
                        }
                    });
                    leaders.push(group[0]);
                }
                leaders.sort_by(|a, b| a.id.cmp(&b.id));
                leaders
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{ContributionMetric, ContributionType, MetricDirection, Verdict};
    use chrono::Duration;

    struct GraphBuilder {
        index: GraphIndex,
        n: usize,
        workflow: String,
    }

    impl GraphBuilder {
        fn new(workflow: &str) -> Self {
            GraphBuilder {
                index: GraphIndex::default(),
                n: 0,
                workflow: workflow.to_string(),
            }
        }

        fn push(
            &mut self,
            record_type: ContributionType,
            actor: &str,
            body: &str,
            parents: Vec<String>,
        ) -> String {
            self.n += 1;
            let mut rec = ContributionRecord::new(record_type, actor, body);
            rec.created = chrono::Utc::now() + Duration::milliseconds(self.n as i64);
            rec.parents = parents;
            rec.workflow = Some(self.workflow.clone());
            let id = rec.compute_id();
            rec.id = id.clone();
            self.index.add(rec);
            id
        }

        fn verify(&mut self, actor: &str, target: &str, verdict: Verdict, body: &str) -> String {
            self.n += 1;
            let mut rec = ContributionRecord::new(ContributionType::Verification, actor, body);
            rec.created = chrono::Utc::now() + Duration::milliseconds(self.n as i64);
            rec.target = Some(target.to_string());
            rec.verdict = Some(verdict);
            rec.parents = vec![target.to_string()];
            rec.workflow = Some(self.workflow.clone());
            let id = rec.compute_id();
            rec.id = id.clone();
            self.index.add(rec);
            id
        }
    }

    #[test]
    fn leaves_exclude_verifications_and_parented_nodes() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let result = g.push(ContributionType::Result, "a", "r1", vec![setup.clone()]);
        let _v = g.verify("b", &result, Verdict::Confirmed, "reproduced");
        let leaves = g.index.view(GraphView::Leaves);
        let ids: Vec<&str> = leaves.iter().map(|r| r.id.as_str()).collect();
        assert!(!ids.contains(&setup.as_str()), "setup has a child");
        assert!(!ids.contains(&result.as_str()), "result has a verification child");
        assert_eq!(leaves.len(), 0, "setup and result both have children; verification excluded");
    }

    #[test]
    fn leaves_include_childless_results() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let r1 = g.push(ContributionType::Result, "a", "r1", vec![setup.clone()]);
        let _r2 = g.push(ContributionType::Result, "a", "r2", vec![r1.clone()]);
        let leaves = g.index.view(GraphView::Leaves);
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].body, "r2");
    }

    #[test]
    fn unverified_respects_failed_and_confirmed_verdicts() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let r1 = g.push(ContributionType::Result, "a", "r1", vec![setup.clone()]);
        let r2 = g.push(ContributionType::Result, "a", "r2", vec![setup.clone()]);
        g.verify("b", &r1, Verdict::Failed, "did not reproduce");
        let unverified: Vec<&str> = g
            .index
            .view(GraphView::Unverified)
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert!(unverified.contains(&r1.as_str()), "failed verdict leaves result unverified");
        assert!(unverified.contains(&r2.as_str()));

        g.verify("c", &r1, Verdict::Confirmed, "reproduced on H100");
        let unverified: Vec<&str> = g
            .index
            .view(GraphView::Unverified)
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert!(!unverified.contains(&r1.as_str()));
    }

    #[test]
    fn newer_verdict_supersedes_older_per_actor() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let r1 = g.push(ContributionType::Result, "a", "r1", vec![setup.clone()]);
        g.verify("b", &r1, Verdict::Confirmed, "first pass");
        g.verify("b", &r1, Verdict::Failed, "second pass failed");
        let verdicts = g.index.effective_verdicts(&r1);
        assert_eq!(verdicts.len(), 1, "one actor -> one effective verdict");
        assert_eq!(verdicts[0].1, Verdict::Failed, "newest verdict wins");
    }

    #[test]
    fn leaders_group_by_metric_and_direction() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let mut r1 = ContributionRecord::new(ContributionType::Result, "a", "good");
        r1.parents = vec![setup.clone()];
        r1.workflow = Some("wf".to_string());
        r1.metric = Some(ContributionMetric {
            name: "bpb".to_string(),
            value: 1.9,
            direction: MetricDirection::Lower,
        });
        r1.created = chrono::Utc::now();
        let r1_id = r1.compute_id();
        r1.id = r1_id.clone();
        g.index.add(r1);

        let mut r2 = ContributionRecord::new(ContributionType::Result, "a", "worse");
        r2.parents = vec![r1_id.clone()];
        r2.workflow = Some("wf".to_string());
        r2.metric = Some(ContributionMetric {
            name: "bpb".to_string(),
            value: 2.5,
            direction: MetricDirection::Lower,
        });
        r2.created = chrono::Utc::now();
        let r2_id = r2.compute_id();
        r2.id = r2_id.clone();
        g.index.add(r2);

        let leaders = g.index.view(GraphView::Leaders);
        assert_eq!(leaders.len(), 1, "one metric group");
        assert_eq!(leaders[0].metric.as_ref().unwrap().value, 1.9, "lower is better");
    }

    #[test]
    fn latest_in_workflow_picks_most_recent() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let r1 = g.push(ContributionType::Result, "a", "r1", vec![setup.clone()]);
        let latest = g.index.latest_in_workflow("wf").unwrap();
        assert_eq!(latest.id, r1);
        assert!(g.index.latest_in_workflow("other").is_none());
    }

    #[test]
    fn open_hypotheses_and_negative_views() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        g.push(ContributionType::Hypothesis, "a", "h1", vec![setup.clone()]);
        let mut neg = ContributionRecord::new(ContributionType::Result, "a", "flopped");
        neg.parents = vec![setup.clone()];
        neg.tags = vec!["negative".to_string()];
        neg.created = chrono::Utc::now();
        neg.workflow = Some("wf".to_string());
        let neg_id = neg.compute_id();
        neg.id = neg_id.clone();
        g.index.add(neg);

        assert_eq!(g.index.view(GraphView::OpenHypotheses).len(), 1);
        let negative = g.index.view(GraphView::Negative);
        assert_eq!(negative.len(), 1);
        assert_eq!(negative[0].body, "flopped");
    }
}
```

- [ ] **Step 2: Register module and run**

In `crates/zerochain-memory/src/lib.rs`: add `pub mod graph_index;` and `pub use graph_index::{GraphIndex, GraphView};`.

Run: `cargo test -p zerochain-memory graph_index 2>&1 | tail -5`
Expected: all 7 tests PASS on the first run. If any fail, fix the implementation — do not weaken the tests.

- [ ] **Step 3: Commit**

```bash
git add crates/zerochain-memory/src/graph_index.rs crates/zerochain-memory/src/lib.rs
git commit -m "feat(memory): add derived graph index with frontier views and verdict supersession"
```

---

### Task 4: Graph — store + index wrapper

**Files:**
- Create: `crates/zerochain-memory/src/graph.rs`
- Modify: `crates/zerochain-memory/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/zerochain-memory/src/graph.rs`:

```rust
use crate::graph_index::GraphIndex;
use crate::graph_store::ContributionStore;
use crate::record::ContributionRecord;
use crate::Result;

/// Workspace-level collective memory: canonical store plus derived index,
/// kept consistent on every publish (spec §4.2).
#[derive(Debug)]
pub struct Graph {
    store: ContributionStore,
    index: GraphIndex,
}

impl Graph {
    pub async fn open(dir: impl AsRef<std::path::Path>) -> Result<Self> {
        let store = ContributionStore::open(&dir).await?;
        let records = store.list().await?;
        Ok(Graph {
            store,
            index: GraphIndex::from_records(records),
        })
    }

    pub async fn publish(&mut self, record: ContributionRecord) -> Result<ContributionRecord> {
        let published = self.store.publish(record).await?;
        self.index.add(published.clone());
        Ok(published)
    }

    pub fn index(&self) -> &GraphIndex {
        &self.index
    }

    pub fn store(&self) -> &ContributionStore {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::ContributionType;
    use tempfile::TempDir;

    #[tokio::test]
    async fn publish_updates_index_and_reopen_rebuilds() {
        let tmp = TempDir::new().unwrap();
        let id = {
            let mut graph = Graph::open(tmp.path().join("graph")).await.unwrap();
            let setup = graph
                .publish(ContributionRecord::new(ContributionType::Setup, "a", "brief"))
                .await
                .unwrap();
            assert_eq!(graph.index().len(), 1);
            setup.id
        };

        let graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        assert_eq!(graph.index().len(), 1);
        assert!(graph.index().get(&id).is_some());
    }

    #[tokio::test]
    async fn publish_failure_leaves_index_unchanged() {
        let tmp = TempDir::new().unwrap();
        let mut graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let mut orphan = ContributionRecord::new(ContributionType::Result, "a", "orphan");
        orphan.parents = vec!["c-missing00000000".to_string()];
        assert!(graph.publish(orphan).await.is_err());
        assert_eq!(graph.index().len(), 0);
    }
}
```

- [ ] **Step 2: Register module and run**

In `crates/zerochain-memory/src/lib.rs`: add `pub mod graph;` and `pub use graph::Graph;`.

Run: `cargo test -p zerochain-memory graph 2>&1 | tail -5`
Expected: all 2 tests PASS (plus earlier graph_store/graph_index tests still pass).

- [ ] **Step 3: Commit**

```bash
git add crates/zerochain-memory/src/graph.rs crates/zerochain-memory/src/lib.rs
git commit -m "feat(memory): add Graph store+index wrapper with rebuild-on-open"
```

---

### Task 5: GraphEmbedIndex — derived semantic search

**Files:**
- Create: `crates/zerochain-memory/src/graph_embed.rs`
- Modify: `crates/zerochain-memory/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/zerochain-memory/src/graph_embed.rs`:

```rust
use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{io_err, MemoryError};
use crate::graph_store::ContributionStore;
use crate::model::EmbeddingModel;
use crate::similarity::cosine_similarity;
use crate::Result;

/// Maximum characters of a record body fed to the embedding model.
const EMBED_BODY_LIMIT: usize = 4000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheLine {
    id: String,
    embedding: Vec<f32>,
}

/// Derived embedding index over record bodies. Persisted as a JSONL cache
/// that is always rebuildable from the canonical store (spec §4.2).
#[derive(Debug, Default)]
pub struct GraphEmbedIndex {
    vectors: HashMap<String, Vec<f32>>,
}

impl GraphEmbedIndex {
    /// Load the cache, embed any uncached records, persist the cache.
    pub async fn build(
        store: &ContributionStore,
        model: &dyn EmbeddingModel,
        cache_path: impl AsRef<Path>,
    ) -> Result<Self> {
        let cache_path = cache_path.as_ref();
        let mut vectors = Self::load_cache(cache_path).await?;
        let records = store.list().await?;
        let pending: Vec<&crate::record::ContributionRecord> = records
            .iter()
            .filter(|r| !vectors.contains_key(&r.id))
            .collect();
        if !pending.is_empty() {
            let texts: Vec<String> = pending
                .iter()
                .map(|r| r.body.chars().take(EMBED_BODY_LIMIT).collect())
                .collect();
            let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
            let embeddings = model.embed(&refs).await?;
            if embeddings.len() != pending.len() {
                return Err(MemoryError::Embedding(format!(
                    "embedding count {} does not match record count {}",
                    embeddings.len(),
                    pending.len()
                )));
            }
            for (record, embedding) in pending.into_iter().zip(embeddings) {
                vectors.insert(record.id.clone(), embedding);
            }
            Self::write_cache(cache_path, &vectors).await?;
        }
        Ok(GraphEmbedIndex { vectors })
    }

    /// Rank candidate record IDs by cosine similarity to the query.
    /// `None` candidates searches all records.
    pub async fn search(
        &self,
        model: &dyn EmbeddingModel,
        query: &str,
        candidates: Option<&std::collections::HashSet<String>>,
        top_k: usize,
    ) -> Result<Vec<String>> {
        let embeddings = model.embed(&[query]).await?;
        let query_vec = embeddings
            .into_iter()
            .next()
            .ok_or_else(|| MemoryError::Embedding("empty query embedding".to_string()))?;
        let mut scored: Vec<(f32, String)> = self
            .vectors
            .iter()
            .filter(|(id, _)| candidates.is_none_or(|c| c.contains(id.as_str())))
            .filter_map(|(id, vec)| cosine_similarity(vec, &query_vec).map(|s| (s, id.clone())))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored.into_iter().map(|(_, id)| id).collect())
    }

    async fn load_cache(path: &Path) -> Result<HashMap<String, Vec<f32>>> {
        let mut vectors = HashMap::new();
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vectors),
            Err(e) => return Err(io_err(path, e)),
        };
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let line: CacheLine = serde_json::from_str(line)?;
            vectors.insert(line.id, line.embedding);
        }
        Ok(vectors)
    }

    async fn write_cache(path: &Path, vectors: &HashMap<String, Vec<f32>>) -> Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| io_err(parent, e))?;
        }
        let mut lines = Vec::new();
        for (id, embedding) in vectors {
            lines.push(serde_json::to_string(&CacheLine {
                id: id.clone(),
                embedding: embedding.clone(),
            })?);
        }
        let tmp = path.with_extension("jsonl.tmp");
        tokio::fs::write(&tmp, lines.join("\n"))
            .await
            .map_err(|e| io_err(&tmp, e))?;
        tokio::fs::rename(&tmp, path)
            .await
            .map_err(|e| io_err(path, e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{ContributionRecord, ContributionType};
    use async_trait::async_trait;
    use tempfile::TempDir;

    /// Deterministic model: embedding = [1,0,0] if text contains "alpha",
    /// else [0,1,0].
    struct KeywordModel;

    #[async_trait]
    impl EmbeddingModel for KeywordModel {
        async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            Ok(texts
                .iter()
                .map(|t| {
                    if t.contains("alpha") {
                        vec![1.0f32, 0.0, 0.0]
                    } else {
                        vec![0.0f32, 1.0, 0.0]
                    }
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn search_ranks_by_cosine_and_filters_candidates() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path().join("contributions")).await.unwrap();
        let a = store
            .publish(ContributionRecord::new(ContributionType::Insight, "a", "alpha approach"))
            .await
            .unwrap();
        let b = store
            .publish(ContributionRecord::new(ContributionType::Insight, "a", "beta approach"))
            .await
            .unwrap();

        let cache = tmp.path().join("index").join("embeddings.jsonl");
        let index = GraphEmbedIndex::build(&store, &KeywordModel, &cache)
            .await
            .unwrap();
        let ranked = index
            .search(&KeywordModel, "alpha", None, 5)
            .await
            .unwrap();
        assert_eq!(ranked[0], a.id);

        let mut candidates = std::collections::HashSet::new();
        candidates.insert(b.id.clone());
        let ranked = index
            .search(&KeywordModel, "alpha", Some(&candidates), 5)
            .await
            .unwrap();
        assert_eq!(ranked, vec![b.id], "candidates filter restricts the pool");
    }

    #[tokio::test]
    async fn cache_persists_and_is_reused() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path().join("contributions")).await.unwrap();
        store
            .publish(ContributionRecord::new(ContributionType::Insight, "a", "alpha"))
            .await
            .unwrap();
        let cache = tmp.path().join("index").join("embeddings.jsonl");
        GraphEmbedIndex::build(&store, &KeywordModel, &cache)
            .await
            .unwrap();
        assert!(cache.exists(), "cache file written");

        // Second build loads from cache; with an empty store it must not
        // re-embed. If it tried, the count check would still pass, so assert
        // the cache round-trips by reusing the index for search.
        let index = GraphEmbedIndex::build(&store, &KeywordModel, &cache)
            .await
            .unwrap();
        let ranked = index.search(&KeywordModel, "alpha", None, 1).await.unwrap();
        assert_eq!(ranked.len(), 1);
    }
}
```

- [ ] **Step 2: Register module and run**

In `crates/zerochain-memory/src/lib.rs`: add `pub mod graph_embed;` and `pub use graph_embed::GraphEmbedIndex;`.

Run: `cargo test -p zerochain-memory graph_embed 2>&1 | tail -5`
Expected: 2 tests PASS. If `is_none_or` errors on the project's Rust version, use `map_or(true, |c| c.contains(*id))` instead.

- [ ] **Step 3: Commit**

```bash
git add crates/zerochain-memory/src/graph_embed.rs crates/zerochain-memory/src/lib.rs
git commit -m "feat(memory): add derived embedding index over contribution bodies"
```

---

### Task 6: Core — stage metric frontmatter + task parents

**Files:**
- Modify: `crates/zerochain-core/src/frontmatter.rs`
- Modify: `crates/zerochain-core/src/task.rs`

- [ ] **Step 1: Write the failing tests**

In `crates/zerochain-core/src/frontmatter.rs`, append to the existing `mod tests`:

```rust
    #[test]
    fn parse_metric_frontmatter() {
        let input = "---\nmetric:\n  name: bpb\n  value: 1.899\n  direction: lower\n---\nBody";
        let frontmatter: ContextFrontmatter = serde_yml::from_str(input).unwrap();
        let metric = frontmatter.metric.expect("metric parsed");
        assert_eq!(metric.name, "bpb");
        assert_eq!(metric.value, 1.899);
        assert_eq!(metric.direction, MetricDirection::Lower);
    }

    #[test]
    fn metric_merges_like_other_optional_fields() {
        let base_yaml = "---\nmetric:\n  name: bpb\n  value: 2.0\n  direction: lower\n---\n";
        let base: ContextFrontmatter = serde_yml::from_str(base_yaml).unwrap();
        let child = ContextFrontmatter::default();
        let merged = child.merge(&base);
        assert_eq!(merged.metric.unwrap().name, "bpb");

        let override_yaml = "---\nmetric:\n  name: loss\n  value: 0.5\n  direction: higher\n---\n";
        let child: ContextFrontmatter = serde_yml::from_str(override_yaml).unwrap();
        let merged = child.merge(&base);
        assert_eq!(merged.metric.unwrap().name, "loss");
    }
```

In `crates/zerochain-core/src/task.rs`, append to the existing `mod tests`:

```rust
    #[test]
    fn parse_task_parents() {
        let input = "---\nid: T1\ntitle: Build on prior run\nparents:\n  - c-aaaaaaaaaaaaaaaa\n  - c-bbbbbbbbbbbbbbbb\n---\nDescription";
        let task = Task::parse(input).unwrap();
        assert_eq!(
            task.parents,
            vec!["c-aaaaaaaaaaaaaaaa".to_string(), "c-bbbbbbbbbbbbbbbb".to_string()]
        );
    }

    #[test]
    fn task_parents_default_empty() {
        let task = Task::parse("---\nid: T2\ntitle: No parents\n---\nBody").unwrap();
        assert!(task.parents.is_empty());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zerochain-core frontmatter task 2>&1 | tail -5`
Expected: compile errors — `metric` field, `MetricDirection`, `StageMetric`, and `Task.parents` do not exist.

- [ ] **Step 3: Implement**

In `crates/zerochain-core/src/frontmatter.rs`, add above `ContextFrontmatter`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricDirection {
    Lower,
    Higher,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageMetric {
    pub name: String,
    pub value: f64,
    pub direction: MetricDirection,
}
```

Add to `ContextFrontmatter` (after `index_output`):

```rust
    #[serde(default)]
    pub metric: Option<StageMetric>,
```

Add to `merge` (after the `index_output` line):

```rust
            metric: self.metric.clone().or(base.metric.clone()),
```

In `crates/zerochain-core/src/task.rs`:
- Add `#[serde(default)] pub parents: Vec<String>,` to `Task` and to `TaskFrontmatter`.
- In `Task::parse`, set `parents: fm.parents` in both the struct literal and (for the `Task::new`/builder path) add a `parents` field default to `TaskBuilder` with a `parents(mut self, Vec<String>)` setter; `build()` includes it. Update `Task::new` to take `parents: Vec<String>` as a parameter... instead, to avoid touching `Task::new` callers, initialize `parents: Vec::new()` in `TaskBuilder::new` and only the builder/parse paths populate it. Check current `Task::new` callers with `grep -rn "Task::new" crates/` — if any exist outside this file, leave `Task::new` signature unchanged and set `parents: Vec::new()` there.

Run: `cargo test -p zerochain-core 2>&1 | tail -5`
Expected: all tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zerochain-core/src/frontmatter.rs crates/zerochain-core/src/task.rs
git commit -m "feat(core): add stage metric frontmatter and task parents"
```

---

## Part 2

Plan continues in the next section with engine integration (Tasks 7–8), tools (Tasks 9–10), human surfaces (Tasks 11–12), and final verification (Task 13).


---

### Task 7: Engine wiring — AppState graph, setup node on init, `parents` plumbing

**Files:**
- Modify: `crates/zerochain-engine/src/state.rs`
- Modify: `crates/zerochain-engine/src/actor.rs`
- Modify: `crates/zerochain-engine/src/registry.rs`
- Modify: `crates/zerochain-daemon/src/mcp.rs`
- Modify: `crates/zerochain-daemon/src/main.rs`
- Modify: `crates/zerochain-server/src/handlers/workflow.rs`
- Test: `crates/zerochain-engine/src/state.rs` (tests module)

- [ ] **Step 1: Verify assumptions**

Run: `grep -n "find_task" crates/zerochain-core/src/workflow.rs | head -3`
Expected: a `pub async fn find_task(&self) -> ...` method. If it returns `Option<Task>`, use it as below; if it returns `Result<Option<Task>>`, adapt with `.await.ok().flatten()`. If it does not exist, skip the task-file branch in Step 4 (body falls back to the workflow id) and note the deviation in the commit message.

Run: `grep -rn "InitWorkflowParams {" crates/ | grep -v target`
Expected: a list of struct-literal sites — every one gains a `parents` field in Step 3.

- [ ] **Step 2: Write the failing tests**

In `crates/zerochain-engine/src/state.rs` tests module, add (imports: `use zerochain_memory::{ContributionRecord, ContributionType, Graph};`):

```rust
    #[tokio::test]
    async fn init_workflow_publishes_setup_node() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        state
            .init_workflow(InitWorkflowParams {
                name: "graph-setup",
                path: None,
                template: Some("00_spec"),
                force: false,
                parents: Vec::new(),
            })
            .await
            .unwrap();

        let graph =
            Graph::open(tmp.path().join(".zerochain").join("graph"))
                .await
                .unwrap();
        let all = graph.index().all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].record_type, ContributionType::Setup);
        assert_eq!(all[0].workflow.as_deref(), Some("graph-setup"));
        assert!(all[0].parents.is_empty());
    }

    #[tokio::test]
    async fn init_workflow_links_declared_parents() {
        let tmp = tempfile::TempDir::new().unwrap();
        let parent_id = {
            let mut graph = Graph::open(tmp.path().join(".zerochain").join("graph"))
                .await
                .unwrap();
            graph
                .publish(ContributionRecord::new(
                    ContributionType::Insight,
                    "prior-agent",
                    "earlier finding",
                ))
                .await
                .unwrap()
                .id
        };
        let mut state = AppState::new(tmp.path(), None).await;
        state
            .init_workflow(InitWorkflowParams {
                name: "graph-child",
                path: None,
                template: Some("00_spec"),
                force: false,
                parents: vec![parent_id.clone()],
            })
            .await
            .unwrap();

        let graph =
            Graph::open(tmp.path().join(".zerochain").join("graph"))
                .await
                .unwrap();
        let setup = graph
            .index()
            .all()
            .into_iter()
            .find(|r| r.record_type == ContributionType::Setup)
            .cloned()
            .expect("setup node");
        assert_eq!(setup.parents, vec![parent_id]);
    }

    #[tokio::test]
    async fn init_workflow_drops_unknown_parents() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        state
            .init_workflow(InitWorkflowParams {
                name: "graph-orphan",
                path: None,
                template: Some("00_spec"),
                force: false,
                parents: vec!["c-unknown00000000".to_string()],
            })
            .await
            .unwrap();
        let graph =
            Graph::open(tmp.path().join(".zerochain").join("graph"))
                .await
                .unwrap();
        let setup = graph.index().all()[0].clone();
        assert!(
            setup.parents.is_empty(),
            "unknown parent filtered, init still succeeds"
        );
    }
```

- [ ] **Step 3: Plumb `parents` through all call sites**

1. `crates/zerochain-engine/src/state.rs`:
   - Imports: extend `use zerochain_memory::{...}` with `ContributionRecord, ContributionType, Graph`; add `use zerochain_core::jj;` if not already imported.
   - `InitWorkflowParams` gains `pub parents: Vec<String>,`.
   - `InitWorkflowRequest` gains `#[serde(default)] pub parents: Vec<String>,`.
   - `AppState` gains field `graph: Option<Arc<TokioMutex<Graph>>>,`.
   - In `AppState::new`, after the `embedding_model` block:

```rust
        let graph = match Graph::open(workspace_root.join(".zerochain").join("graph")).await {
            Ok(graph) => Some(Arc::new(TokioMutex::new(graph))),
            Err(e) => {
                tracing::warn!(error = %e, "failed to open contribution graph; graph features unavailable");
                None
            }
        };
```

   and add `graph,` to the struct literal.
   - `clone_state()`: add `graph: self.graph.clone(),`.
   - Add accessor:

```rust
    pub async fn graph(&self) -> Option<Arc<TokioMutex<Graph>>> {
        self.graph.clone()
    }
```

2. `crates/zerochain-engine/src/actor.rs`: `ActorMessage::InitWorkflow` gains `parents: Vec<String>`; the `params` construction gains `parents,`; `WorkflowHandle::init_workflow` gains a `parents: Vec<String>` parameter passed into the message.
3. `crates/zerochain-engine/src/registry.rs`: `init_workflow(&self, name, template, parents)` passes `parents` through; update its two test call sites to `registry.init_workflow("alpha".into(), None, vec![])` (and `"beta"`).
4. `crates/zerochain-daemon/src/mcp.rs`: destructure `parents` in the `InitWorkflowRequest` pattern (replace `..` with `parents, ..`) and pass to `registry.init_workflow(name, template, parents)`.
5. `crates/zerochain-daemon/src/main.rs`: `Commands::Init` match arm gains `parents` binding; params construction gains `parents` (pass `Vec::new()` for now — Task 11 wires the CLI flag). If `cli.rs` has no `parents` field yet, this arm binds it from the new CLI field added in Task 11 — to keep the build green in this task, add the CLI field now (see Step 5 of Task 11) or bind `parents: Vec::new()` here and revise in Task 11. **Choose: add the CLI field now** (`parents: Vec<String>` with `#[arg(long = "parent")]` on `Commands::Init`) and pass it through.
6. `crates/zerochain-server/src/handlers/workflow.rs`: `registry.init_workflow(body.name, body.template, body.parents).await`.
7. Every remaining `InitWorkflowParams { ... }` literal (state.rs tests, llm_driver.rs tests, engine/tests/*.rs — the grep from Step 1): add `parents: Vec::new(),`.

- [ ] **Step 4: Publish the setup node in `init_workflow`**

At the end of `AppState::init_workflow`, after `self.workflows.insert(...)` and before `Ok(workflow)`:

```rust
        if let Some(graph) = self.graph().await {
            let task = workflow.find_task().await;
            let mut setup_parents: Vec<String> = parents;
            if let Some(task) = &task {
                setup_parents.extend(task.parents.iter().cloned());
            }
            setup_parents.dedup();

            let body = task
                .as_ref()
                .map(|t| {
                    if t.description.trim().is_empty() {
                        t.title.clone()
                    } else {
                        t.description.clone()
                    }
                })
                .unwrap_or_else(|| workflow.id.clone());

            let mut locked = graph.lock().await;
            let existing: std::collections::HashSet<String> = locked
                .index()
                .all()
                .iter()
                .map(|r| r.id.clone())
                .collect();
            let kept: Vec<String> = setup_parents
                .into_iter()
                .filter(|p| {
                    let ok = existing.contains(p);
                    if !ok {
                        tracing::warn!(parent = %p, "dropping unknown parent contribution");
                    }
                    ok
                })
                .collect();

            let mut record = ContributionRecord::new(
                ContributionType::Setup,
                zerochain_core::okf::zerochain_actor(),
                body,
            );
            record.parents = kept;
            record.workflow = Some(workflow.id.clone());
            match locked.publish(record).await {
                Ok(published) => {
                    drop(locked);
                    jj::auto_commit(
                        &self.workspace_root,
                        &format!("graph: setup {}", published.id),
                    )
                    .await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to publish setup contribution");
                }
            }
        }

        Ok(workflow)
```

Note: `parents` was destructured from params at the top of `init_workflow` — keep the binding instead of discarding.

- [ ] **Step 5: Run tests**

Run: `cargo test -p zerochain-engine 2>&1 | tail -5`
Expected: all tests PASS, including the 3 new setup-node tests. Fix any remaining `InitWorkflowParams` literals the compiler flags.

- [ ] **Step 6: Commit**

```bash
git add crates/zerochain-engine crates/zerochain-daemon crates/zerochain-server crates/zerochain-core
git commit -m "feat(engine): open workspace graph, publish setup node on workflow init"
```

---

### Task 8: Result auto-capture from stage completion

**Files:**
- Modify: `crates/zerochain-engine/src/llm_driver.rs`

- [ ] **Step 1: Write the failing tests**

In `crates/zerochain-engine/src/llm_driver.rs` tests module, add:

```rust
    #[tokio::test]
    async fn publishes_result_contribution_chained_to_setup() {
        let tmp = TempDir::new().unwrap();
        let state = test_state_with_embedding(&tmp).await;
        let mut state_mut = state.clone_state();
        let wf = state_mut
            .init_workflow(crate::state::InitWorkflowParams {
                name: "graph-chain",
                path: None,
                template: Some("00_spec"),
                force: false,
                parents: Vec::new(),
            })
            .await
            .unwrap();
        let state = Arc::new(state_mut);
        let ctx_path = wf.root.join("00_spec").join("CONTEXT.md");
        tokio::fs::write(
            &ctx_path,
            "---\nindex_output: true\nmetric:\n  name: bpb\n  value: 1.899\n  direction: lower\n---\nEvaluate.",
        )
        .await
        .unwrap();

        let stage = Stage::from_dir(&wf.root.join("00_spec")).await.unwrap();
        let llm = FakeLlm {
            response: "bpb 1.899".into(),
        };
        let mut workflows = HashMap::new();
        workflows.insert(wf.id.clone(), wf);
        let driver = LLMStageDriver {
            workflow_id: "graph-chain",
            stage: &stage,
            llm: &llm,
            cas: None,
            context_cache: None,
            tool_registry: Arc::new(ToolRegistry::default()),
            state: state.clone(),
        };
        driver.execute(&mut workflows).await.unwrap();

        let graph = zerochain_memory::Graph::open(
            tmp.path().join(".zerochain").join("graph"),
        )
        .await
        .unwrap();
        let all = graph.index().all();
        let setup = all
            .iter()
            .find(|r| r.record_type == zerochain_memory::ContributionType::Setup)
            .cloned()
            .expect("setup node");
        let result = all
            .iter()
            .find(|r| r.record_type == zerochain_memory::ContributionType::Result)
            .cloned()
            .expect("result node");
        assert_eq!(result.parents, vec![setup.id]);
        assert_eq!(result.workflow.as_deref(), Some("graph-chain"));
        assert_eq!(result.stage.as_deref(), Some("00_spec"));
        let metric = result.metric.expect("metric captured");
        assert_eq!(metric.name, "bpb");
        assert!((metric.value - 1.899).abs() < 1e-9);
    }

    #[tokio::test]
    async fn no_result_record_without_index_output() {
        let tmp = TempDir::new().unwrap();
        let state = test_state_with_embedding(&tmp).await;
        let mut state_mut = state.clone_state();
        let wf = state_mut
            .init_workflow(crate::state::InitWorkflowParams {
                name: "graph-no-index",
                path: None,
                template: Some("00_spec"),
                force: false,
                parents: Vec::new(),
            })
            .await
            .unwrap();
        let state = Arc::new(state_mut);
        let ctx_path = wf.root.join("00_spec").join("CONTEXT.md");
        tokio::fs::write(&ctx_path, "---\nindex_output: false\n---\nEvaluate.")
            .await
            .unwrap();

        let stage = Stage::from_dir(&wf.root.join("00_spec")).await.unwrap();
        let llm = FakeLlm {
            response: "plain output".into(),
        };
        let mut workflows = HashMap::new();
        workflows.insert(wf.id.clone(), wf);
        let driver = LLMStageDriver {
            workflow_id: "graph-no-index",
            stage: &stage,
            llm: &llm,
            cas: None,
            context_cache: None,
            tool_registry: Arc::new(ToolRegistry::default()),
            state: state.clone(),
        };
        driver.execute(&mut workflows).await.unwrap();

        let graph = zerochain_memory::Graph::open(
            tmp.path().join(".zerochain").join("graph"),
        )
        .await
        .unwrap();
        assert_eq!(
            graph.index().all().len(),
            1,
            "only the setup node; index_output disabled"
        );
    }
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p zerochain-engine llm_driver 2>&1 | tail -5`
Expected: the new tests FAIL (no result node published — the method does not exist yet).

- [ ] **Step 3: Implement `publish_result_contribution`**

In `crates/zerochain-engine/src/llm_driver.rs`:

1. In `execute`, immediately after `self.index_output(&ctx, &output).await?;`:

```rust
        if let Err(e) = self.publish_result_contribution(&ctx, &output).await {
            tracing::warn!(error = %e, "failed to publish result contribution");
        }
```

2. Add to the second `impl<'a> LLMStageDriver<'a>` block (alongside `index_output`):

```rust
    /// Publish a `result` contribution for this stage's output, parented on the
    /// workflow's most recent contribution (spec §5). Best-effort: failures
    /// never fail the stage (spec §7).
    async fn publish_result_contribution(
        &self,
        ctx: &Option<StageContext>,
        output: &str,
    ) -> Result<(), DaemonError> {
        let Some(graph) = self.state.graph().await else {
            return Ok(());
        };
        let Some(ctx) = ctx else {
            return Ok(());
        };
        if !ctx.frontmatter.index_output || output.is_empty() {
            return Ok(());
        }

        let parents = {
            let graph = graph.lock().await;
            graph
                .index()
                .latest_in_workflow(self.workflow_id)
                .map(|r| vec![r.id.clone()])
                .unwrap_or_default()
        };
        let metric = ctx.frontmatter.metric.clone().map(|m| {
            zerochain_memory::ContributionMetric {
                name: m.name,
                value: m.value,
                direction: match m.direction {
                    zerochain_core::frontmatter::MetricDirection::Lower => {
                        zerochain_memory::MetricDirection::Lower
                    }
                    zerochain_core::frontmatter::MetricDirection::Higher => {
                        zerochain_memory::MetricDirection::Higher
                    }
                },
            }
        });
        let artifacts = match &self.cas {
            Some(cas) => match cas.put(output.as_bytes()).await {
                Ok(cid) => vec![format!("{cid}")],
                Err(e) => {
                    tracing::warn!(error = %e, "failed to store output in CAS");
                    Vec::new()
                }
            },
            None => Vec::new(),
        };

        let mut record = ContributionRecord::new(
            ContributionType::Result,
            zerochain_core::okf::zerochain_actor(),
            output,
        );
        record.parents = parents;
        record.workflow = Some(self.workflow_id.to_string());
        record.stage = Some(self.stage.id.raw.clone());
        record.metric = metric;
        record.artifacts = artifacts;

        let published = {
            let mut graph = graph.lock().await;
            graph.publish(record).await
        };
        match published {
            Ok(record) => {
                zerochain_core::jj::auto_commit(
                    &self.state.workspace_root,
                    &format!("graph: result {}", record.id),
                )
                .await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to publish result contribution");
            }
        }
        Ok(())
    }
```

3. Extend imports: add `ContributionRecord, ContributionType` to the `zerochain_memory` import.

- [ ] **Step 4: Run tests**

Run: `cargo test -p zerochain-engine llm_driver 2>&1 | tail -5`
Expected: all tests PASS, including the pre-existing `indexes_stage_output_when_index_output_is_true` (chunk indexing retained alongside graph publishing).

- [ ] **Step 5: Commit**

```bash
git add crates/zerochain-engine/src/llm_driver.rs
git commit -m "feat(engine): auto-capture result contributions chained to workflow lineage"
```

---

### Task 9: LLM tools — contribute, verify, graph_query

**Files:**
- Create: `crates/zerochain-tools/src/graph_tool.rs`
- Modify: `crates/zerochain-tools/src/registry.rs`
- Modify: `crates/zerochain-tools/src/lib.rs`

- [ ] **Step 1: Write the implementation with colocated tests**

Create `crates/zerochain-tools/src/graph_tool.rs`:

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use zerochain_error::{Result, ZerochainError};
use zerochain_memory::{
    ContributionMetric, ContributionRecord, ContributionType, Graph, GraphEmbedIndex, GraphView,
    MetricDirection, Verdict,
};

use crate::tool::Tool;

const DEFAULT_TOP_K: usize = 5;

pub struct ContributeTool;

#[async_trait]
impl Tool for ContributeTool {
    fn name(&self) -> &str {
        "contribute"
    }

    fn description(&self) -> &str {
        "Publish a typed contribution (insight, hypothesis, or report) to the workspace contribution graph."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "type": { "type": "string", "enum": ["insight", "hypothesis", "report"], "description": "Contribution type." },
                "body": { "type": "string", "description": "Markdown body of the contribution." },
                "parents": { "type": "array", "items": { "type": "string" }, "description": "Parent contribution IDs. Defaults to the latest contribution in the current workflow." },
                "tags": { "type": "array", "items": { "type": "string" } },
                "metric": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "value": { "type": "number" },
                        "direction": { "type": "string", "enum": ["lower", "higher"] }
                    },
                    "required": ["name", "value", "direction"]
                },
                "graph_dir": { "type": "string", "description": "Injected by the engine; do not set manually." },
                "graph_workflow": { "type": "string", "description": "Injected by the engine; do not set manually." },
                "graph_stage": { "type": "string", "description": "Injected by the engine; do not set manually." },
                "graph_actor": { "type": "string", "description": "Injected by the engine; do not set manually." }
            },
            "required": ["type", "body"]
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let record_type = ContributionType::parse(required_str(&input, "type")?).map_err(|_| {
            ZerochainError::InvalidInput {
                message: "type must be insight, hypothesis, or report".to_string(),
            }
        })?;
        if !matches!(
            record_type,
            ContributionType::Insight | ContributionType::Hypothesis | ContributionType::Report
        ) {
            return Err(ZerochainError::InvalidInput {
                message: "contribute type must be insight, hypothesis, or report".to_string(),
            });
        }
        let body = required_str(&input, "body")?.to_string();
        let graph_dir = required_str(&input, "graph_dir")?;
        let workflow = optional_str(&input, "graph_workflow");
        let actor = optional_str(&input, "graph_actor").unwrap_or_else(|| "zerochain/unknown".into());

        let mut graph = Graph::open(graph_dir).await?;
        let parents = match input.get("parents").and_then(Value::as_array) {
            Some(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            None => workflow
                .as_deref()
                .and_then(|wf| graph.index().latest_in_workflow(wf).map(|r| vec![r.id.clone()]))
                .unwrap_or_default(),
        };
        let mut record = ContributionRecord::new(record_type, actor, body);
        record.parents = parents;
        record.workflow = workflow;
        record.metric = parse_metric(input.get("metric"))?;
        record.tags = string_array(input.get("tags"));
        let published = graph.publish(record).await?;
        Ok(json!({ "id": published.id }))
    }
}

pub struct VerifyTool;

#[async_trait]
impl Tool for VerifyTool {
    fn name(&self) -> &str {
        "verify"
    }

    fn description(&self) -> &str {
        "Publish a verification verdict (confirmed, partial, or failed) for a contribution in the workspace graph."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": { "type": "string", "description": "Contribution ID to verify." },
                "verdict": { "type": "string", "enum": ["confirmed", "partial", "failed"] },
                "body": { "type": "string", "description": "Evidence for the verdict." },
                "graph_dir": { "type": "string", "description": "Injected by the engine; do not set manually." },
                "graph_workflow": { "type": "string", "description": "Injected by the engine; do not set manually." },
                "graph_actor": { "type": "string", "description": "Injected by the engine; do not set manually." }
            },
            "required": ["target", "verdict", "body"]
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let target = required_str(&input, "target")?.to_string();
        let verdict = Verdict::parse(required_str(&input, "verdict")?)
            .map_err(|e| ZerochainError::InvalidInput { message: e.to_string() })?;
        let body = required_str(&input, "body")?.to_string();
        let graph_dir = required_str(&input, "graph_dir")?;
        let actor = optional_str(&input, "graph_actor").unwrap_or_else(|| "zerochain/unknown".into());

        let mut graph = Graph::open(graph_dir).await?;
        let mut record = ContributionRecord::new(ContributionType::Verification, actor, body);
        record.target = Some(target.clone());
        record.verdict = Some(verdict);
        record.parents = vec![target];
        record.workflow = optional_str(&input, "graph_workflow");
        let published = graph.publish(record).await?;
        Ok(json!({ "id": published.id }))
    }
}

pub struct GraphQueryTool;

#[async_trait]
impl Tool for GraphQueryTool {
    fn name(&self) -> &str {
        "graph_query"
    }

    fn description(&self) -> &str {
        "Search the workspace contribution graph semantically, or list a named view (recent, leaves, open_hypotheses, unverified, negative, leaders)."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Semantic search text over contribution bodies." },
                "view": { "type": "string", "enum": ["recent", "leaves", "open_hypotheses", "unverified", "negative", "leaders"] },
                "type": { "type": "string", "description": "Filter by contribution type." },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter: records carrying all listed tags." },
                "workflow": { "type": "string", "description": "Filter by workflow." },
                "top_k": { "type": "number", "description": "Max results (default 5)." },
                "graph_dir": { "type": "string", "description": "Injected by the engine; do not set manually." }
            }
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let graph_dir = required_str(&input, "graph_dir")?;
        let top_k = input
            .get("top_k")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_TOP_K);
        let graph = Graph::open(graph_dir).await?;

        let view = match input.get("view").and_then(Value::as_str) {
            Some(s) => Some(GraphView::parse(s).ok_or_else(|| {
                ZerochainError::InvalidInput { message: format!("unknown view: {s}") }
            })?),
            None => None,
        };
        let record_type = input
            .get("type")
            .and_then(Value::as_str)
            .map(ContributionType::parse)
            .transpose()
            .map_err(|e| ZerochainError::InvalidInput { message: e.to_string() })?;
        let workflow = optional_str(&input, "workflow");
        let tags = string_array(input.get("tags"));

        let mut records: Vec<ContributionRecord> = match view {
            Some(v) => graph.index().view(v).into_iter().cloned().collect(),
            None => graph.index().all().into_iter().cloned().collect(),
        };
        if let Some(t) = record_type {
            records.retain(|r| r.record_type == t);
        }
        if let Some(wf) = &workflow {
            records.retain(|r| r.workflow.as_deref() == Some(wf.as_str()));
        }
        if !tags.is_empty() {
            records.retain(|r| tags.iter().all(|t| r.tags.contains(t)));
        }

        let ordered: Vec<ContributionRecord> =
            match input.get("query").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                Some(query) => {
                    let model =
                        tokio::task::spawn_blocking(zerochain_memory::FastEmbedModel::try_new)
                            .await
                            .map_err(|e| ZerochainError::Other { message: e.to_string() })?
                            .map_err(|e| ZerochainError::Other {
                                message: format!("failed to initialize embedding model: {e}"),
                            })?;
                    let cache = std::path::Path::new(graph_dir)
                        .join("index")
                        .join("embeddings.jsonl");
                    let embeds = GraphEmbedIndex::build(graph.store(), &model, &cache).await?;
                    let candidates: std::collections::HashSet<String> =
                        records.iter().map(|r| r.id.clone()).collect();
                    let ranked = embeds.search(&model, query, Some(&candidates), top_k).await?;
                    let mut by_id: std::collections::HashMap<String, ContributionRecord> =
                        records.into_iter().map(|r| (r.id.clone(), r)).collect();
                    ranked.into_iter().filter_map(|id| by_id.remove(&id)).collect()
                }
                None => {
                    records.sort_by(|a, b| b.created.cmp(&a.created));
                    records.truncate(top_k);
                    records
                }
            };

        let results: Vec<Value> = ordered.iter().map(record_json).collect();
        Ok(json!({ "results": results }))
    }
}

fn record_json(r: &ContributionRecord) -> Value {
    json!({
        "id": r.id,
        "type": r.record_type.as_str(),
        "parents": r.parents,
        "actor": r.actor,
        "created": r.created.to_rfc3339(),
        "workflow": r.workflow,
        "metric": r.metric.as_ref().map(|m| {
            json!({
                "name": m.name,
                "value": m.value,
                "direction": serde_json::to_value(m.direction).unwrap_or(Value::Null),
            })
        }),
        "excerpt": excerpt(&r.body),
    })
}

fn excerpt(body: &str) -> String {
    let flat: String = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join(" ");
    flat.chars().take(240).collect()
}

fn required_str<'v>(input: &'v Value, key: &str) -> Result<&'v str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ZerochainError::InvalidInput {
            message: format!("missing '{key}' field"),
        })
}

fn optional_str(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::to_string)
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn parse_metric(value: Option<&Value>) -> Result<Option<ContributionMetric>> {
    let Some(v) = value else { return Ok(None) };
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ZerochainError::InvalidInput {
            message: "metric requires 'name'".to_string(),
        })?;
    let value = v
        .get("value")
        .and_then(Value::as_f64)
        .ok_or_else(|| ZerochainError::InvalidInput {
            message: "metric requires numeric 'value'".to_string(),
        })?;
    let direction = MetricDirection::parse(
        v.get("direction")
            .and_then(Value::as_str)
            .ok_or_else(|| ZerochainError::InvalidInput {
                message: "metric requires 'direction'".to_string(),
            })?,
    )
    .map_err(|e| ZerochainError::InvalidInput { message: e.to_string() })?;
    Ok(Some(ContributionMetric {
        name: name.to_string(),
        value,
        direction,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn base_input(tmp: &TempDir) -> Value {
        json!({
            "graph_dir": tmp.path().join("graph").to_str().unwrap(),
            "graph_workflow": "wf",
            "graph_actor": "zerochain/test"
        })
    }

    #[tokio::test]
    async fn contribute_publishes_with_default_parent() {
        let tmp = TempDir::new().unwrap();
        let mut graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let setup = graph
            .publish(ContributionRecord::new(ContributionType::Setup, "a", "root"))
            .await
            .unwrap();

        let tool = ContributeTool;
        let mut input = base_input(&tmp);
        input["type"] = json!("insight");
        input["body"] = json!("donor ensembling helps");
        let result = tool.run(input).await.unwrap();
        let id = result.get("id").and_then(Value::as_str).unwrap().to_string();

        let graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let rec = graph.index().get(&id).unwrap().clone();
        assert_eq!(rec.parents, vec![setup.id]);
        assert_eq!(rec.workflow.as_deref(), Some("wf"));
    }

    #[tokio::test]
    async fn contribute_rejects_result_type() {
        let tmp = TempDir::new().unwrap();
        let tool = ContributeTool;
        let mut input = base_input(&tmp);
        input["type"] = json!("result");
        input["body"] = json!("x");
        assert!(tool.run(input).await.is_err());
    }

    #[tokio::test]
    async fn verify_publishes_and_rejects_bad_target() {
        let tmp = TempDir::new().unwrap();
        let mut graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let target = graph
            .publish(ContributionRecord::new(ContributionType::Result, "a", "measured"))
            .await
            .unwrap()
            .id;

        let tool = VerifyTool;
        let mut input = base_input(&tmp);
        input["target"] = json!(target);
        input["verdict"] = json!("confirmed");
        input["body"] = json!("bit-identical reproduction");
        let result = tool.run(input).await.unwrap();
        let vid = result.get("id").and_then(Value::as_str).unwrap().to_string();

        let graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let v = graph.index().get(&vid).unwrap().clone();
        assert_eq!(v.record_type, ContributionType::Verification);
        assert_eq!(v.parents, vec![target.clone()]);

        let mut bad = base_input(&tmp);
        bad["target"] = json!("c-missing00000000");
        bad["verdict"] = json!("confirmed");
        bad["body"] = json!("x");
        assert!(tool.run(bad).await.is_err());
    }

    #[tokio::test]
    async fn graph_query_lists_views_without_embeddings() {
        let tmp = TempDir::new().unwrap();
        let mut graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let setup = graph
            .publish(ContributionRecord::new(ContributionType::Setup, "a", "root"))
            .await
            .unwrap();
        let mut result =
            ContributionRecord::new(ContributionType::Result, "a", "measured thing");
        result.parents = vec![setup.id.clone()];
        result.workflow = Some("wf".to_string());
        result.metric = Some(ContributionMetric {
            name: "bpb".into(),
            value: 1.9,
            direction: MetricDirection::Lower,
        });
        graph.publish(result).await.unwrap();

        let tool = GraphQueryTool;
        let mut input = base_input(&tmp);
        input["view"] = json!("leaders");
        let out = tool.run(input).await.unwrap();
        let results = out.get("results").and_then(Value::as_array).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].get("type").and_then(Value::as_str), Some("result"));
    }
}
```

- [ ] **Step 2: Register tools and run**

In `crates/zerochain-tools/src/registry.rs` `Default` impl, add:

```rust
        registry.register(Arc::new(crate::graph_tool::ContributeTool));
        registry.register(Arc::new(crate::graph_tool::VerifyTool));
        registry.register(Arc::new(crate::graph_tool::GraphQueryTool));
```

In `crates/zerochain-tools/src/lib.rs`: add `pub mod graph_tool;` and re-export `pub use graph_tool::{ContributeTool, GraphQueryTool, VerifyTool};`.

Run: `cargo test -p zerochain-tools graph_tool 2>&1 | tail -5`
Expected: all 4 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/zerochain-tools/src/graph_tool.rs crates/zerochain-tools/src/registry.rs crates/zerochain-tools/src/lib.rs
git commit -m "feat(tools): add contribute, verify, and graph_query tools"
```

---

### Task 10: Engine injection + tool-loop integration

**Files:**
- Modify: `crates/zerochain-engine/src/tool_driver.rs`
- Modify: `crates/zerochain-engine/src/llm_driver.rs`
- Test: `crates/zerochain-engine/tests/graph_tools.rs`

- [ ] **Step 1: Write the failing integration test**

Create `crates/zerochain-engine/tests/graph_tools.rs`:

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;
use zerochain_engine::state::{AppState, InitWorkflowParams};
use zerochain_llm::{
    CompleteResponse, FinishReason, LLMConfig, Message, ProviderId, Tool, ToolCall, LLM,
};
use zerochain_memory::{ContributionType, Graph};

struct FakeEmbed;

#[async_trait]
impl zerochain_memory::EmbeddingModel for FakeEmbed {
    async fn embed(
        &self,
        texts: &[&str],
    ) -> std::result::Result<Vec<Vec<f32>>, zerochain_memory::MemoryError> {
        Ok(texts.iter().map(|_| vec![1.0f32, 0.0, 0.0]).collect())
    }
}

struct GraphToolLlm {
    calls: AtomicUsize,
}

fn last_json(messages: &[Message]) -> Value {
    let text = messages
        .last()
        .and_then(|m| m.content.text())
        .unwrap_or_default();
    let start = text.find('{').expect("tool result JSON in message");
    serde_json::from_str(&text[start..]).expect("valid tool result JSON")
}

#[async_trait]
impl LLM for GraphToolLlm {
    fn provider_id(&self) -> &ProviderId {
        static PROVIDER: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        PROVIDER.get_or_init(|| ProviderId::OpenAI)
    }

    async fn complete(
        &self,
        _config: &LLMConfig,
        messages: &[Message],
        tools: Option<&[Tool]>,
    ) -> std::result::Result<CompleteResponse, zerochain_llm::error::LLMError> {
        let tools = tools.expect("tools should be passed to the LLM");
        assert!(tools.iter().any(|t| t.name == "contribute"));
        assert!(tools.iter().any(|t| t.name == "verify"));
        assert!(tools.iter().any(|t| t.name == "graph_query"));

        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut response = CompleteResponse::new(None);
        response.model = "mock".into();

        match call {
            0 => {
                response.tool_calls = vec![ToolCall::new(
                    "call_1",
                    "contribute",
                    json!({
                        "type": "insight",
                        "body": "donor ensembling improves transfer"
                    }),
                )];
                response.finish_reason = FinishReason::ToolCalls;
            }
            1 => {
                let id = last_json(messages)
                    .get("id")
                    .and_then(Value::as_str)
                    .expect("contribute returns an id")
                    .to_string();
                response.tool_calls = vec![ToolCall::new(
                    "call_2",
                    "verify",
                    json!({
                        "target": id,
                        "verdict": "confirmed",
                        "body": "reproduced on a second run"
                    }),
                )];
                response.finish_reason = FinishReason::ToolCalls;
            }
            _ => {
                let content = messages
                    .last()
                    .and_then(|m| m.content.text())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                response.content = Some(content);
            }
        }

        Ok(response)
    }

    fn supports_multimodal(&self) -> bool {
        false
    }

    fn context_window(&self) -> usize {
        4096
    }

    async fn health_check(&self) -> std::result::Result<(), zerochain_llm::error::LLMError> {
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[tokio::test]
async fn graph_tools_publish_and_verify_through_tool_loop() {
    let tmp = TempDir::new().unwrap();
    let mut state = AppState::new(tmp.path(), None).await;
    state.embedding_model = Some(Arc::new(FakeEmbed));

    let wf = state
        .init_workflow(InitWorkflowParams {
            name: "graph-tools-wf",
            path: None,
            template: Some("00_spec"),
            force: false,
            parents: Vec::new(),
        })
        .await
        .unwrap();

    let stage = wf.stages[0].clone();
    tokio::fs::write(
        &stage.context_path,
        "---\nrole: graph tool runner\ntools:\n  - contribute\n  - verify\n  - graph_query\n---\nUse the graph tools.\n",
    )
    .await
    .unwrap();

    let llm = GraphToolLlm {
        calls: AtomicUsize::new(0),
    };
    state
        .execute_stage_with_llm("graph-tools-wf", &stage, &llm)
        .await
        .unwrap();

    let result = tokio::fs::read_to_string(stage.output_path.join("result.md"))
        .await
        .unwrap();
    assert!(result.contains("\"id\""), "final output carries tool result; got: {result}");

    let graph = Graph::open(tmp.path().join(".zerochain").join("graph"))
        .await
        .unwrap();
    let verifications: Vec<_> = graph
        .index()
        .all()
        .into_iter()
        .filter(|r| r.record_type == ContributionType::Verification)
        .cloned()
        .collect();
    assert_eq!(verifications.len(), 1, "one verification published");
    let insights: Vec<_> = graph
        .index()
        .all()
        .into_iter()
        .filter(|r| r.record_type == ContributionType::Insight)
        .cloned()
        .collect();
    assert_eq!(insights.len(), 1);
    assert_eq!(verifications[0].target, Some(insights[0].id.clone()));
    assert_eq!(
        insights[0].workflow.as_deref(),
        Some("graph-tools-wf"),
        "workflow context injected"
    );
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test -p zerochain-engine --test graph_tools 2>&1 | tail -5`
Expected: FAIL — the graph tools are not in the passed tool list unless listed in CONTEXT.md (they are), but `contribute` fails at runtime: `missing 'graph_dir' field` (no injection yet).

- [ ] **Step 3: Implement injection**

In `crates/zerochain-engine/src/tool_driver.rs`:

1. Add the context struct and extend `execute_tool_call`:

```rust
/// Graph context injected into graph tool calls (contribute/verify/graph_query).
#[derive(Debug, Clone)]
pub struct GraphInvokeContext {
    pub workflow_id: String,
    pub stage_id: String,
    pub actor: String,
    pub graph_dir: std::path::PathBuf,
}
```

2. Add a `graph: Option<&GraphInvokeContext>` parameter to `execute_tool_call` and the injection block after the existing `memory_store_path` block:

```rust
    if let Some(g) = graph {
        if matches!(call.name.as_str(), "contribute" | "verify" | "graph_query") {
            input["graph_dir"] = serde_json::json!(g.graph_dir.to_string_lossy().to_string());
            input["graph_workflow"] = serde_json::json!(g.workflow_id);
            input["graph_stage"] = serde_json::json!(g.stage_id);
            input["graph_actor"] = serde_json::json!(g.actor);
        }
    }
```

3. In `crates/zerochain-engine/src/llm_driver.rs` `execute`: before the tool loop, build the context once:

```rust
        let graph_ctx = Some(tool_driver::GraphInvokeContext {
            workflow_id: self.workflow_id.to_string(),
            stage_id: self.stage.id.raw.clone(),
            actor: zerochain_core::okf::zerochain_actor(),
            graph_dir: self
                .state
                .workspace_root
                .join(".zerochain")
                .join("graph"),
        });
```

(`graph_ctx` is unconditional: `Option` only for signature compatibility.)

4. Pass `graph_ctx.as_ref()` at both `execute_tool_call` call sites.

Run: `cargo test -p zerochain-engine --test graph_tools 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 4: Check for other callers and commit**

Run: `grep -rn "execute_tool_call" crates/ | grep -v target`
Expected: only `tool_driver.rs` (definition) and `llm_driver.rs` (two call sites). Fix any others the compiler flags.

Run: `cargo test -p zerochain-engine 2>&1 | tail -3` — all PASS.

```bash
git add crates/zerochain-engine/src/tool_driver.rs crates/zerochain-engine/src/llm_driver.rs crates/zerochain-engine/tests/graph_tools.rs
git commit -m "feat(engine): inject graph context into LLM tool calls"
```

---

### Task 11: CLI — init --parent, contribute, verify, graph

**Files:**
- Modify: `crates/zerochain-daemon/src/cli.rs`
- Modify: `crates/zerochain-daemon/src/main.rs`
- Modify: `crates/zerochain-daemon/Cargo.toml`
- Test: `crates/zerochain-daemon/tests/cli_graph_parse.rs`

- [ ] **Step 1: Write the failing parse tests**

Create `crates/zerochain-daemon/tests/cli_graph_parse.rs`:

```rust
use clap::Parser;
use zerochain_daemon::cli::{Cli, Commands};

#[test]
fn parse_contribute_command() {
    let cli = Cli::try_parse_from([
        "zerochain",
        "--workspace",
        "/tmp/ws",
        "contribute",
        "--type",
        "insight",
        "--body",
        "found a pattern",
        "--parent",
        "c-abc",
        "--tag",
        "negative",
        "--metric",
        "name=bpb,value=1.9,direction=lower",
    ])
    .unwrap();
    match cli.command {
        Commands::Contribute {
            kind,
            body,
            parents,
            tags,
            metric,
        } => {
            assert_eq!(kind, "insight");
            assert_eq!(body, "found a pattern");
            assert_eq!(parents, vec!["c-abc".to_string()]);
            assert_eq!(tags, vec!["negative".to_string()]);
            assert!(metric.is_some());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_verify_command() {
    let cli = Cli::try_parse_from([
        "zerochain",
        "verify",
        "c-target00000000",
        "--verdict",
        "confirmed",
        "--body",
        "reproduced",
    ])
    .unwrap();
    match cli.command {
        Commands::Verify {
            target, verdict, ..
        } => {
            assert_eq!(target, "c-target00000000");
            assert_eq!(verdict, "confirmed");
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_graph_command() {
    let cli =
        Cli::try_parse_from(["zerochain", "graph", "--view", "leaders", "--json"]).unwrap();
    match cli.command {
        Commands::Graph { view, json, .. } => {
            assert_eq!(view.as_deref(), Some("leaders"));
            assert!(json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_init_parents() {
    let cli = Cli::try_parse_from([
        "zerochain",
        "init",
        "--name",
        "w",
        "--parent",
        "c-a",
        "--parent",
        "c-b",
    ])
    .unwrap();
    match cli.command {
        Commands::Init { parents, .. } => {
            assert_eq!(parents, vec!["c-a".to_string(), "c-b".to_string()])
        }
        other => panic!("unexpected command: {other:?}"),
    }
}
```

In `crates/zerochain-daemon/Cargo.toml` `[dev-dependencies]` add `clap = { workspace = true, features = ["derive"] }` and in `[dependencies]` add `zerochain-memory = { path = "../zerochain-memory" }` and `serde_json.workspace = true` if absent.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p zerochain-daemon --test cli_graph_parse 2>&1 | tail -5`
Expected: compile errors — `Contribute`/`Verify`/`Graph` variants don't exist.

- [ ] **Step 3: Add CLI variants and dispatch**

In `crates/zerochain-daemon/src/cli.rs`:

1. `Commands::Init` gains:

```rust
        #[arg(long = "parent", help = "Parent contribution ID to build on (repeatable)")]
        parents: Vec<String>,
```

2. New variants (before `Mcp`):

```rust
    #[command(about = "Publish a contribution to the workspace graph")]
    Contribute {
        #[arg(long = "type", help = "Contribution type: insight, hypothesis, or report")]
        kind: String,
        #[arg(short, long, help = "Markdown body of the contribution")]
        body: String,
        #[arg(long = "parent", help = "Parent contribution ID (repeatable)")]
        parents: Vec<String>,
        #[arg(long = "tag", help = "Tag (repeatable)")]
        tags: Vec<String>,
        #[arg(long, help = "Metric spec: name=bpb,value=1.9,direction=lower")]
        metric: Option<String>,
    },
    #[command(about = "Publish a verification verdict for a contribution")]
    Verify {
        #[arg(help = "Target contribution ID")]
        target: String,
        #[arg(long, help = "Verdict: confirmed, partial, or failed")]
        verdict: String,
        #[arg(short, long, help = "Evidence body")]
        body: String,
    },
    #[command(about = "Inspect the workspace contribution graph")]
    Graph {
        #[arg(long, help = "View: recent, leaves, open_hypotheses, unverified, negative, leaders")]
        view: Option<String>,
        #[arg(long = "type", help = "Filter by contribution type")]
        record_type: Option<String>,
        #[arg(long, help = "Filter by workflow")]
        workflow: Option<String>,
        #[arg(long, help = "Emit JSON")]
        json: bool,
    },
```

In `crates/zerochain-daemon/src/main.rs`:

1. Add helpers near the top:

```rust
fn human_actor() -> String {
    let id = std::env::var("ZEROCHAIN_OKF_ACTOR")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_string());
    format!("human:{id}")
}

fn parse_metric_spec(spec: &str) -> anyhow::Result<zerochain_memory::ContributionMetric> {
    let mut name = None;
    let mut value = None;
    let mut direction = None;
    for part in spec.split(',') {
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("metric spec must be key=value pairs: {spec}"))?;
        match k.trim() {
            "name" => name = Some(v.trim().to_string()),
            "value" => {
                value = Some(
                    v.trim()
                        .parse::<f64>()
                        .map_err(|e| anyhow::anyhow!("metric value: {e}"))?,
                )
            }
            "direction" => direction = Some(v.trim().to_string()),
            other => return Err(anyhow::anyhow!("unknown metric key: {other}")),
        }
    }
    Ok(zerochain_memory::ContributionMetric {
        name: name.ok_or_else(|| anyhow::anyhow!("metric requires name"))?,
        value: value.ok_or_else(|| anyhow::anyhow!("metric requires value"))?,
        direction: zerochain_memory::MetricDirection::parse(
            &direction.ok_or_else(|| anyhow::anyhow!("metric requires direction"))?,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?,
    })
}
```

2. Update the `Init` arm to bind `parents` and pass `parents` into `InitWorkflowParams` (Task 7 added the field; this wires the flag).

3. Add dispatch arms:

```rust
        zerochain_daemon::cli::Commands::Contribute {
            kind,
            body,
            parents,
            tags,
            metric,
        } => {
            let record_type = zerochain_memory::ContributionType::parse(&kind)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if record_type == zerochain_memory::ContributionType::Verification {
                return Err(anyhow::anyhow!(
                    "use the verify command for verification records"
                ));
            }
            let graph_dir = cli.workspace.join(".zerochain").join("graph");
            let mut graph = zerochain_memory::Graph::open(&graph_dir).await?;
            let mut record =
                zerochain_memory::ContributionRecord::new(record_type, human_actor(), body);
            record.parents = parents;
            record.metric = metric
                .as_deref()
                .map(parse_metric_spec)
                .transpose()?;
            record.tags = tags;
            let published = graph.publish(record).await?;
            zerochain_core::jj::auto_commit(
                &cli.workspace,
                &format!("graph: {} {}", record_type.as_str(), published.id),
            )
            .await;
            println!("published contribution: {}", published.id);
        }
        zerochain_daemon::cli::Commands::Verify {
            target,
            verdict,
            body,
        } => {
            let verdict = zerochain_memory::Verdict::parse(&verdict)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let graph_dir = cli.workspace.join(".zerochain").join("graph");
            let mut graph = zerochain_memory::Graph::open(&graph_dir).await?;
            let mut record = zerochain_memory::ContributionRecord::new(
                zerochain_memory::ContributionType::Verification,
                human_actor(),
                body,
            );
            record.target = Some(target.clone());
            record.verdict = Some(verdict);
            record.parents = vec![target];
            let published = graph.publish(record).await?;
            zerochain_core::jj::auto_commit(
                &cli.workspace,
                &format!("graph: verification {}", published.id),
            )
            .await;
            println!("published verification: {}", published.id);
        }
        zerochain_daemon::cli::Commands::Graph {
            view,
            record_type,
            workflow,
            json,
        } => {
            let graph_dir = cli.workspace.join(".zerochain").join("graph");
            let graph = zerochain_memory::Graph::open(&graph_dir).await?;
            let view = view
                .as_deref()
                .map(|s| {
                    zerochain_memory::GraphView::parse(s)
                        .ok_or_else(|| anyhow::anyhow!("unknown view: {s}"))
                })
                .transpose()?;
            let record_type = record_type
                .as_deref()
                .map(|s| {
                    zerochain_memory::ContributionType::parse(s)
                        .map_err(|e| anyhow::anyhow!("{e}"))
                })
                .transpose()?;
            let mut records: Vec<zerochain_memory::ContributionRecord> = match view {
                Some(v) => graph.index().view(v).into_iter().cloned().collect(),
                None => graph.index().all().into_iter().cloned().collect(),
            };
            if let Some(t) = record_type {
                records.retain(|r| r.record_type == t);
            }
            if let Some(wf) = &workflow {
                records.retain(|r| r.workflow.as_deref() == Some(wf.as_str()));
            }
            if json {
                let out: Vec<serde_json::Value> = records.iter().map(record_json).collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                for r in &records {
                    let first_line = r
                        .body
                        .lines()
                        .map(str::trim)
                        .find(|l| !l.is_empty())
                        .unwrap_or("");
                    println!("{}\t{}\t{}\t{}", r.id, r.record_type.as_str(), r.actor, first_line);
                }
            }
        }
```

4. Add the local `record_json` helper (same shape as the tool's):

```rust
fn record_json(r: &zerochain_memory::ContributionRecord) -> serde_json::Value {
    serde_json::json!({
        "id": r.id,
        "type": r.record_type.as_str(),
        "parents": r.parents,
        "actor": r.actor,
        "created": r.created.to_rfc3339(),
        "workflow": r.workflow,
        "stage": r.stage,
        "tags": r.tags,
        "metric": r.metric.as_ref().map(|m| serde_json::json!({
            "name": m.name,
            "value": m.value,
            "direction": serde_json::to_value(m.direction).unwrap_or(serde_json::Value::Null),
        })),
    })
}
```

Note: if `zerochain-core` is not in daemon `[dependencies]`, add it (main.rs already imports `zerochain_core::stage::StageId`, so it is).

- [ ] **Step 4: Run tests**

Run: `cargo test -p zerochain-daemon 2>&1 | tail -5`
Expected: all tests PASS (new parse tests + existing integration tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zerochain-daemon
git commit -m "feat(cli): add contribute, verify, graph commands and init --parent"
```

---

### Task 12: HTTP surface + shared filter helper

**Files:**
- Modify: `crates/zerochain-memory/src/graph_index.rs` (add `filtered`)
- Modify: `crates/zerochain-tools/src/graph_tool.rs` (refactor to `filtered`)
- Modify: `crates/zerochain-server/src/state.rs`
- Create: `crates/zerochain-server/src/handlers/graph.rs`
- Modify: `crates/zerochain-server/src/handlers/mod.rs`
- Modify: `crates/zerochain-server/Cargo.toml`
- Test: `crates/zerochain-server/tests/integration.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/zerochain-server/tests/integration.rs`:

```rust
#[tokio::test]
async fn graph_contribute_and_query_round_trip() {
    let tmp = TempDir::new().unwrap();
    let app = make_app(tmp.path()).await;

    let req = make_request(
        "POST",
        "/v1/graph/contributions",
        Some(r#"{"type":"insight","body":"cross-run insight","actor":"human:tester"}"#),
    );
    let resp = send!(app.clone(), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request("GET", "/v1/graph?view=recent", None);
    let resp = send!(app, req);
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp.into_body()).await;
    assert!(
        body.contains("cross-run insight"),
        "view should include the contribution; got: {body}"
    );
}

#[tokio::test]
async fn graph_verification_updates_unverified_view() {
    let tmp = TempDir::new().unwrap();
    let app = make_app(tmp.path()).await;

    let req = make_request(
        "POST",
        "/v1/graph/contributions",
        Some(r#"{"type":"setup","body":"project brief","workflow":"w1"}"#),
    );
    let resp = send!(app.clone(), req);
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_string(resp.into_body()).await;
    let target: serde_json::Value = serde_json::from_str(&body).unwrap();
    let target = target.get("id").and_then(|v| v.as_str()).unwrap().to_string();

    // setup is not a result, so unverified stays empty; publish a result.
    let req = make_request(
        "POST",
        "/v1/graph/contributions",
        Some(&format!(
            r#"{{"type":"result","body":"measured","workflow":"w1","parents":["{target}"]}}"#
        )),
    );
    let resp = send!(app.clone(), req);
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_string(resp.into_body()).await;
    let result: serde_json::Value = serde_json::from_str(&body).unwrap();
    let result_id = result.get("id").and_then(|v| v.as_str()).unwrap().to_string();

    let req = make_request("GET", "/v1/graph?view=unverified", None);
    let resp = send!(app.clone(), req);
    let body = body_string(resp.into_body()).await;
    assert!(body.contains(&result_id), "result is unverified; got: {body}");

    let req = make_request(
        "POST",
        "/v1/graph/verifications",
        Some(&format!(
            r#"{{"target":"{result_id}","verdict":"confirmed","body":"reproduced"}}"#
        )),
    );
    let resp = send!(app.clone(), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request("GET", "/v1/graph?view=unverified", None);
    let resp = send!(app, req);
    let body = body_string(resp.into_body()).await;
    assert!(
        !body.contains(&result_id),
        "verified result leaves the unverified view; got: {body}"
    );
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p zerochain-server graph 2>&1 | tail -5`
Expected: 404s — routes don't exist.

- [ ] **Step 3: Add `GraphIndex::filtered` and refactor the tool**

In `crates/zerochain-memory/src/graph_index.rs` `impl GraphIndex`, add:

```rust
    /// View plus type/workflow/tag filters, shared by tools, CLI, and HTTP.
    pub fn filtered(
        &self,
        view: Option<GraphView>,
        record_type: Option<ContributionType>,
        workflow: Option<&str>,
        tags: &[String],
    ) -> Vec<&ContributionRecord> {
        let base = match view {
            Some(v) => self.view(v),
            None => self.all(),
        };
        base.into_iter()
            .filter(|r| record_type.is_none_or(|t| r.record_type == t))
            .filter(|r| workflow.is_none_or(|w| r.workflow.as_deref() == Some(w)))
            .filter(|r| tags.iter().all(|t| r.tags.contains(t)))
            .collect()
    }
```

Add a test in the same file's test module:

```rust
    #[test]
    fn filtered_combines_view_and_filters() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        g.push(ContributionType::Hypothesis, "a", "h1", vec![setup.clone()]);
        let mut other = ContributionRecord::new(ContributionType::Hypothesis, "a", "h2-other-wf");
        other.workflow = Some("other".to_string());
        other.created = chrono::Utc::now();
        let other_id = other.compute_id();
        other.id = other_id.clone();
        g.index.add(other);

        let recs = g.index.filtered(
            Some(GraphView::OpenHypotheses),
            None,
            Some("wf"),
            &[],
        );
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].body, "h1");
    }
```

In `crates/zerochain-tools/src/graph_tool.rs` `GraphQueryTool::run`, replace the manual view/filter block with:

```rust
        let workflow_filter = optional_str(&input, "workflow");
        let records: Vec<ContributionRecord> = graph
            .index()
            .filtered(view, record_type, workflow_filter.as_deref(), &tags)
            .into_iter()
            .cloned()
            .collect();
```

(If `is_none_or` is unavailable on the pinned toolchain, use `.map_or(true, |t| r.record_type == t)` style closures.)

- [ ] **Step 4: ServerState graph + handlers + routes**

1. `crates/zerochain-server/Cargo.toml` `[dependencies]`: add `zerochain-memory = { path = "../zerochain-memory" }`, `serde_json.workspace = true`, and `tokio = { workspace = true, features = ["sync"] }` if absent.

2. `crates/zerochain-server/src/state.rs`: add field `pub graph: Option<Arc<tokio::sync::Mutex<zerochain_memory::Graph>>>`; in `ServerState::new`:

```rust
        let graph = match zerochain_memory::Graph::open(workspace.join(".zerochain").join("graph")).await {
            Ok(g) => Some(Arc::new(tokio::sync::Mutex::new(g))),
            Err(e) => {
                tracing::warn!(error = %e, "failed to open contribution graph");
                None
            }
        };
```

and `graph,` in the struct literal.

3. Create `crates/zerochain-server/src/handlers/graph.rs`:

```rust
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use zerochain_memory::{ContributionRecord, ContributionType, GraphView, Verdict};

use crate::handlers::SimpleMessage;
use crate::state::ServerState;

#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    pub view: Option<String>,
    #[serde(rename = "type")]
    pub record_type: Option<String>,
    pub workflow: Option<String>,
    pub tags: Option<String>,
    pub query: Option<String>,
    pub top_k: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ContributionHttpRequest {
    #[serde(rename = "type")]
    pub record_type: String,
    pub body: String,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub metric: Option<HttpMetric>,
    pub workflow: Option<String>,
    pub stage: Option<String>,
    pub actor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HttpMetric {
    pub name: String,
    pub value: f64,
    pub direction: String,
}

#[derive(Debug, Deserialize)]
pub struct VerificationHttpRequest {
    pub target: String,
    pub verdict: String,
    pub body: String,
    pub actor: Option<String>,
    pub workflow: Option<String>,
}

fn record_json(r: &ContributionRecord) -> serde_json::Value {
    serde_json::json!({
        "id": r.id,
        "type": r.record_type.as_str(),
        "parents": r.parents,
        "actor": r.actor,
        "created": r.created.to_rfc3339(),
        "workflow": r.workflow,
        "stage": r.stage,
        "tags": r.tags,
        "metric": r.metric.as_ref().map(|m| serde_json::json!({
            "name": m.name,
            "value": m.value,
            "direction": serde_json::to_value(m.direction).unwrap_or(serde_json::Value::Null),
        })),
    })
}

fn unavailable() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(SimpleMessage {
            message: "contribution graph unavailable".into(),
        }),
    )
        .into_response()
}

pub async fn query(
    State(state): State<ServerState>,
    Query(params): Query<GraphQueryParams>,
) -> impl IntoResponse {
    let Some(graph) = state.graph.clone() else {
        return unavailable();
    };
    let view = match params.view.as_deref() {
        Some(s) => match GraphView::parse(s) {
            Some(v) => Some(v),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(SimpleMessage {
                        message: format!("unknown view: {s}"),
                    }),
                )
                    .into_response()
            }
        },
        None => None,
    };
    let record_type = match params.record_type.as_deref() {
        Some(s) => match ContributionType::parse(s) {
            Ok(t) => Some(t),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(SimpleMessage {
                        message: e.to_string(),
                    }),
                )
                    .into_response()
            }
        },
        None => None,
    };
    let tags: Vec<String> = params
        .tags
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let graph = graph.lock().await;
    let mut records: Vec<ContributionRecord> = graph
        .index()
        .filtered(view, record_type, params.workflow.as_deref(), &tags)
        .into_iter()
        .cloned()
        .collect();

    match params.query.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(q) => {
            let model = match tokio::task::spawn_blocking(
                zerochain_memory::FastEmbedModel::try_new,
            )
            .await
            {
                Ok(Ok(m)) => m,
                Ok(Err(e)) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(SimpleMessage {
                            message: format!("embedding model: {e}"),
                        }),
                    )
                        .into_response()
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(SimpleMessage {
                            message: e.to_string(),
                        }),
                    )
                        .into_response()
                }
            };
            let cache = state
                .workspace
                .join(".zerochain")
                .join("graph")
                .join("index")
                .join("embeddings.jsonl");
            let embeds = match zerochain_memory::GraphEmbedIndex::build(
                graph.store(),
                &model,
                &cache,
            )
            .await
            {
                Ok(e) => e,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(SimpleMessage {
                            message: e.to_string(),
                        }),
                    )
                        .into_response()
                }
            };
            let candidates: std::collections::HashSet<String> =
                records.iter().map(|r| r.id.clone()).collect();
            let top_k = params.top_k.unwrap_or(10) as usize;
            records = match embeds.search(&model, q, Some(&candidates), top_k).await {
                Ok(ranked) => {
                    let mut by_id: std::collections::HashMap<String, ContributionRecord> =
                        records.into_iter().map(|r| (r.id.clone(), r)).collect();
                    ranked
                        .into_iter()
                        .filter_map(|id| by_id.remove(&id))
                        .collect()
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(SimpleMessage {
                            message: e.to_string(),
                        }),
                    )
                        .into_response()
                }
            };
        }
        None => {
            records.sort_by(|a, b| b.created.cmp(&a.created));
            records.truncate(params.top_k.unwrap_or(50) as usize);
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "results": records.iter().map(record_json).collect::<Vec<_>>()
        })),
    )
        .into_response()
}

pub async fn contribute(
    State(state): State<ServerState>,
    Json(body): Json<ContributionHttpRequest>,
) -> impl IntoResponse {
    let Some(graph) = state.graph.clone() else {
        return unavailable();
    };
    let record_type = match ContributionType::parse(&body.record_type) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SimpleMessage {
                    message: e.to_string(),
                }),
            )
                .into_response()
        }
    };
    let mut record = ContributionRecord::new(
        record_type,
        body.actor.unwrap_or_else(|| "human:api".to_string()),
        body.body,
    );
    record.parents = body.parents;
    record.tags = body.tags;
    record.workflow = body.workflow;
    record.stage = body.stage;
    record.metric = match body.metric {
        Some(m) => match zerochain_memory::MetricDirection::parse(&m.direction) {
            Ok(direction) => Some(zerochain_memory::ContributionMetric {
                name: m.name,
                value: m.value,
                direction,
            }),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(SimpleMessage {
                        message: e.to_string(),
                    }),
                )
                    .into_response()
            }
        },
        None => None,
    };

    let mut graph = graph.lock().await;
    match graph.publish(record).await {
        Ok(published) => {
            let id = published.id.clone();
            drop(graph);
            zerochain_core::jj::auto_commit(
                &state.workspace,
                &format!("graph: {} {id}", record_type.as_str()),
            )
            .await;
            (
                StatusCode::CREATED,
                Json(serde_json::json!({ "id": id })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(SimpleMessage {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn verify(
    State(state): State<ServerState>,
    Json(body): Json<VerificationHttpRequest>,
) -> impl IntoResponse {
    let Some(graph) = state.graph.clone() else {
        return unavailable();
    };
    let verdict = match Verdict::parse(&body.verdict) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SimpleMessage {
                    message: e.to_string(),
                }),
            )
                .into_response()
        }
    };
    let mut record = ContributionRecord::new(
        ContributionType::Verification,
        body.actor.unwrap_or_else(|| "human:api".to_string()),
        body.body,
    );
    record.target = Some(body.target.clone());
    record.verdict = Some(verdict);
    record.parents = vec![body.target];
    record.workflow = body.workflow;

    let mut graph = graph.lock().await;
    match graph.publish(record).await {
        Ok(published) => {
            let id = published.id.clone();
            drop(graph);
            zerochain_core::jj::auto_commit(
                &state.workspace,
                &format!("graph: verification {id}"),
            )
            .await;
            (
                StatusCode::CREATED,
                Json(serde_json::json!({ "id": id })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(SimpleMessage {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}
```

4. `crates/zerochain-server/src/handlers/mod.rs`: add `pub mod graph;` and routes inside `protected`:

```rust
        .route("/v1/graph", get(graph::query))
        .route("/v1/graph/contributions", post(graph::contribute))
        .route("/v1/graph/verifications", post(graph::verify))
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p zerochain-server 2>&1 | tail -5` and `cargo test -p zerochain-tools 2>&1 | tail -3` and `cargo test -p zerochain-memory 2>&1 | tail -3`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zerochain-server crates/zerochain-memory crates/zerochain-tools
git commit -m "feat(server): expose contribution graph over HTTP with shared filters"
```

---

### Task 13: Docs + final verification

**Files:**
- Modify: `README.md`
- Modify: `crates/zerochain-memory/src/lib.rs` (crate doc line)

- [ ] **Step 1: Update docs**

In `README.md`, after the "Open Knowledge Format (OKF) v0.2" section, add:

```markdown
---

## 🕸️ Collective Contribution Graph

zerochain keeps a workspace-level, append-only graph of typed contributions so workflows build on past runs instead of starting from scratch. Every workflow init publishes a `setup` node; stages with `index_output: true` publish `result` nodes chained to their lineage; agents publish `insight`/`hypothesis`/`verification` records via the `contribute`, `verify`, and `graph_query` tools (list them in a stage's `tools:` frontmatter). Records live as content-addressed markdown under `.zerochain/graph/` and are auditable through the same jj trail as everything else.

\`\`\`bash
# Link a new workflow to prior contributions
zerochain init --name run-2 --parent c-9f3a21c7d4e8b601

# Human surfaces
zerochain contribute --type insight --body "donor ensembling helps" --parent c-9f3a21c7d4e8b601
zerochain verify c-9f3a21c7d4e8b601 --verdict confirmed --body "reproduced on H100"
zerochain graph --view leaders        # recent | leaves | open_hypotheses | unverified | negative | leaders
\`\`\`

Stage outputs can carry a metric via CONTEXT.md frontmatter: \`metric: {name: bpb, value: 1.899, direction: lower}\`.
```

In `crates/zerochain-memory/src/lib.rs`, update the crate doc to:

```rust
//! Filesystem-native collective memory for zerochain: a typed, append-only
//! contribution graph with derived semantic search, plus the legacy
//! per-workflow vector store.
```

- [ ] **Step 2: Full verification**

Run: `cargo fmt && cargo test --workspace 2>&1 | tail -10`
Expected: all test suites PASS. Note: `zerochain-tools` tests and any semantic-query tests use the real `FastEmbedModel` and download model weights on first run (~90 MB to `~/.cache/zerochain/models`); if the network is unavailable, those specific tests fail with a download error — that is environmental, not a code defect.

Run: `cargo clippy --workspace --all-targets 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 3: Commit**

```bash
git add README.md crates/zerochain-memory/src/lib.rs
git commit -m "docs: document the collective contribution graph"
```

---

## Self-review notes

- Spec §3 (data model) → Tasks 1–2. §4.2 (storage/index) → Tasks 2–5. §5 (engine) → Tasks 7–8. §6.1 (tools) → Tasks 9–10. §6.2 (CLI) → Task 11. §6.3 (HTTP) → Task 12. §7 (error handling: atomic writes, corrupt-file skip, publish-failure-never-fails-stage) → Tasks 2, 8. §8 (testing) → per-task test steps. §9 (deferred) — no tasks, by design.
- Known simplifications, all consistent with the spec's phase-1 scope: verification records use `parents: [target]` (keeps `children` maps meaningful); `id` truncation to 64 bits with collision check on publish; derived indexes converge on reopen (last-write-wins, spec §4.4).
