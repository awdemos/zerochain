use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{io_err, MemoryError};
use crate::graph_store::ContributionStore;
use crate::model::EmbeddingModel;
use crate::similarity::cosine_similarity;
use crate::Result;

/// Process-global counter disambiguating concurrent tmp files: two writers
/// in this process must never share one tmp name (a fixed name lets
/// concurrent cache writes interleave bytes or fail each other's rename).
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    /// `None` candidates searches all records. Returned ids are not validated
    /// against the store; pass `candidates` to restrict to live records.
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
        let mut scored: Vec<(f32, String)> = Vec::new();
        for (id, vec) in self.vectors.iter() {
            if !candidates.is_none_or(|c| c.contains(id.as_str())) {
                continue;
            }
            match cosine_similarity(vec, &query_vec) {
                Some(s) => scored.push((s, id.clone())),
                None => {
                    tracing::warn!(record_id = %id, "embedding dimension mismatch; record excluded from search")
                }
            }
        }
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });
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
            // Self-healing: a corrupt line is skipped (and its record
            // re-embedded on the next build), never fatal.
            match serde_json::from_str::<CacheLine>(line) {
                Ok(line) => {
                    vectors.insert(line.id, line.embedding);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "skipping corrupt embedding cache line")
                }
            }
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
        let tmp = path.with_extension(format!(
            "jsonl.tmp.{}.{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
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
        let store = ContributionStore::open(tmp.path().join("contributions"))
            .await
            .unwrap();
        let a = store
            .publish(ContributionRecord::new(
                ContributionType::Insight,
                "a",
                "alpha approach",
            ))
            .await
            .unwrap();
        let b = store
            .publish(ContributionRecord::new(
                ContributionType::Insight,
                "a",
                "beta approach",
            ))
            .await
            .unwrap();

        let cache = tmp.path().join("index").join("embeddings.jsonl");
        let index = GraphEmbedIndex::build(&store, &KeywordModel, &cache)
            .await
            .unwrap();
        let ranked = index.search(&KeywordModel, "alpha", None, 5).await.unwrap();
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
        let store = ContributionStore::open(tmp.path().join("contributions"))
            .await
            .unwrap();
        store
            .publish(ContributionRecord::new(
                ContributionType::Insight,
                "a",
                "alpha",
            ))
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

    struct PanicModel;

    #[async_trait]
    impl EmbeddingModel for PanicModel {
        async fn embed(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            panic!("embed must not be called when the cache is complete");
        }
    }

    #[tokio::test]
    async fn build_uses_cache_without_embedding() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path().join("contributions"))
            .await
            .unwrap();
        store
            .publish(ContributionRecord::new(
                ContributionType::Insight,
                "a",
                "alpha",
            ))
            .await
            .unwrap();
        let cache = tmp.path().join("index").join("embeddings.jsonl");
        GraphEmbedIndex::build(&store, &KeywordModel, &cache)
            .await
            .unwrap();
        // Second build: cache is complete, the model must not be invoked.
        let index = GraphEmbedIndex::build(&store, &PanicModel, &cache)
            .await
            .unwrap();
        let ranked = index.search(&KeywordModel, "alpha", None, 1).await.unwrap();
        assert_eq!(ranked.len(), 1);
    }

    #[tokio::test]
    async fn concurrent_cache_builds_do_not_clobber_each_other() {
        let tmp = TempDir::new().unwrap();
        let store = ContributionStore::open(tmp.path().join("contributions"))
            .await
            .unwrap();
        store
            .publish(ContributionRecord::new(
                ContributionType::Insight,
                "a",
                "alpha",
            ))
            .await
            .unwrap();
        let cache = tmp.path().join("index").join("embeddings.jsonl");

        // Both builds write the cache concurrently; with a fixed tmp name
        // one rename removes the other's tmp file and the build fails.
        let (r1, r2) = tokio::join!(
            GraphEmbedIndex::build(&store, &KeywordModel, &cache),
            GraphEmbedIndex::build(&store, &KeywordModel, &cache)
        );
        r1.unwrap();
        r2.unwrap();
        let index = GraphEmbedIndex::build(&store, &KeywordModel, &cache)
            .await
            .unwrap();
        let ranked = index.search(&KeywordModel, "alpha", None, 1).await.unwrap();
        assert_eq!(ranked.len(), 1);
    }
}
