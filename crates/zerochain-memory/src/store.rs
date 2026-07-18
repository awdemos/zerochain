use std::path::{Path, PathBuf};

use crate::error::{io_err, MemoryError};
use crate::model::{EmbeddingModel, MemoryChunk};
use crate::similarity::cosine_similarity;
use crate::Result;

/// On-disk vector memory store for a single workflow.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    chunks: Vec<MemoryChunk>,
    path: PathBuf,
}

impl MemoryStore {
    /// Create or load a store at the given directory.
    pub async fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| io_err(&dir, e))?;
        let path = dir.join("memory.jsonl");
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(MemoryStore {
                chunks: Vec::new(),
                path,
            });
        }
        let data = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| io_err(&path, e))?;
        let mut chunks = Vec::new();
        for line in data.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let chunk: MemoryChunk = serde_json::from_str(line)?;
            chunks.push(chunk);
        }
        Ok(MemoryStore { chunks, path })
    }

    /// Add chunks to the store after embedding them.
    pub async fn add(
        &mut self,
        model: &dyn EmbeddingModel,
        texts: Vec<(String, serde_json::Value)>,
    ) -> Result<usize> {
        if texts.is_empty() {
            return Ok(0);
        }
        let refs: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        let embeddings = model.embed(&refs).await?;
        if embeddings.len() != texts.len() {
            return Err(MemoryError::InvalidInput(format!(
                "embedding count {} does not match text count {}",
                embeddings.len(),
                texts.len()
            )));
        }
        let mut added = 0;
        for ((text, metadata), embedding) in texts.into_iter().zip(embeddings) {
            let id = format!("chunk-{}", self.chunks.len());
            let mut chunk = MemoryChunk::new(id, text, metadata);
            chunk.embedding = embedding;
            self.chunks.push(chunk);
            added += 1;
        }
        self.persist().await?;
        Ok(added)
    }

    /// Search for the top-K chunks most similar to the query embedding.
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(f32, MemoryChunk)>> {
        let mut scored: Vec<(f32, MemoryChunk)> = self
            .chunks
            .iter()
            .filter_map(|chunk| {
                cosine_similarity(&chunk.embedding, query).map(|score| (score, chunk.clone()))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored)
    }

    /// Embed a query and return top-K matching chunks.
    pub async fn query(
        &self,
        model: &dyn EmbeddingModel,
        query_text: &str,
        top_k: usize,
    ) -> Result<Vec<(f32, MemoryChunk)>> {
        let embeddings = model.embed(&[query_text]).await?;
        let query = embeddings
            .into_iter()
            .next()
            .ok_or_else(|| MemoryError::InvalidInput("empty query embedding".into()))?;
        self.search(&query, top_k)
    }

    async fn persist(&self) -> Result<()> {
        let mut lines = Vec::new();
        for chunk in &self.chunks {
            let line = serde_json::to_string(chunk)?;
            lines.push(line);
        }
        let content = lines.join("\n");
        let tmp_path = self.path.with_extension("jsonl.tmp");
        tokio::fs::write(&tmp_path, content)
            .await
            .map_err(|e| io_err(&tmp_path, e))?;
        tokio::fs::rename(&tmp_path, &self.path)
            .await
            .map_err(|e| io_err(&self.path, e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use tempfile::TempDir;

    struct FixedModel;

    #[async_trait]
    impl EmbeddingModel for FixedModel {
        async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            Ok(texts
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let mut v = vec![0.0f32; 3];
                    v[i % 3] = 1.0;
                    v
                })
                .collect())
        }
    }

    struct FakeEmbeddingModel;

    #[async_trait]
    impl EmbeddingModel for FakeEmbeddingModel {
        async fn embed(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            Ok(_texts.iter().map(|_| vec![1.0f32, 0.0, 0.0]).collect())
        }
    }

    #[tokio::test]
    async fn add_and_search_store() {
        let tmp = TempDir::new().unwrap();
        let mut store = MemoryStore::open(tmp.path()).await.unwrap();
        let model = FixedModel;
        store
            .add(
                &model,
                vec![
                    ("first".into(), serde_json::json!({})),
                    ("second".into(), serde_json::json!({})),
                ],
            )
            .await
            .unwrap();
        let results = store.search(&[1.0f32, 0.0, 0.0], 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.text, "first");
    }

    #[tokio::test]
    async fn memory_store_persists_across_reopen() {
        let dir = TempDir::new().unwrap();
        let model = FakeEmbeddingModel;

        {
            let mut store = MemoryStore::open(dir.path()).await.unwrap();
            store
                .add(
                    &model,
                    vec![("zerochain semantic search".into(), json!({"id": 1}))],
                )
                .await
                .unwrap();
        }

        let store2 = MemoryStore::open(dir.path()).await.unwrap();
        let results = store2.query(&model, "semantic", 5).await.unwrap();
        assert!(!results.is_empty());
        assert!(results
            .iter()
            .any(|r| r.1.text.contains("zerochain semantic search")));
    }
}
