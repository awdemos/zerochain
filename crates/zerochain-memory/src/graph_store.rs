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
        let computed = record.compute_id();
        if record.id.is_empty() {
            record.id = computed;
        } else if record.id != computed {
            return Err(MemoryError::InvalidInput(format!(
                "record id {} does not match content",
                record.id
            )));
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
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| io_err(&self.dir, e))?
        {
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
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "skipping corrupt contribution record")
                    }
                },
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "skipping unreadable contribution record")
                }
            }
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Verdict;
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
        assert_eq!(
            count, 1,
            "identical re-publish must not write a second file"
        );
    }

    #[tokio::test]
    async fn publish_rejects_mismatched_caller_supplied_id() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path()).await.unwrap();
        let mut rec = ContributionRecord::new(ContributionType::Insight, "a", "content");
        rec.id = "c-forged000000000".to_string();
        assert!(store.publish(rec).await.is_err());
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
            .publish(ContributionRecord::new(
                ContributionType::Setup,
                "a",
                "root",
            ))
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
            .publish(ContributionRecord::new(
                ContributionType::Result,
                "a",
                "measured",
            ))
            .await
            .unwrap();
        let mut verification =
            ContributionRecord::new(ContributionType::Verification, "b", "reproduced");
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
            .publish(ContributionRecord::new(
                ContributionType::Insight,
                "a",
                "good",
            ))
            .await
            .unwrap();
        tokio::fs::write(
            tmp.path().join("c-corrupt0000000.md"),
            "not markdown at all",
        )
        .await
        .unwrap();
        let records = store.list().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].body, "good");
    }

    #[tokio::test]
    async fn publish_rejects_collision_with_different_content() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path()).await.unwrap();
        let rec = ContributionRecord::new(ContributionType::Insight, "a", "original");
        let first = store.publish(rec.clone()).await.unwrap();
        let path = tmp.path().join(format!("{}.md", first.id));
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        tokio::fs::write(&path, content.replace("original", "tampered"))
            .await
            .unwrap();
        let err = store.publish(rec).await.unwrap_err();
        assert!(
            err.to_string().contains("collision"),
            "expected collision error, got: {err}"
        );
    }

    #[tokio::test]
    async fn publish_rejects_verification_of_missing_target() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path()).await.unwrap();
        let mut rec = ContributionRecord::new(ContributionType::Verification, "a", "verdict");
        rec.target = Some("c-missingtarget00".to_string());
        rec.verdict = Some(Verdict::Confirmed);
        rec.parents = vec![];
        let err = store.publish(rec).await.unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "expected target-not-found error, got: {err}"
        );
    }
}
