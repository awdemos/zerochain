use async_trait::async_trait;
use fastembed::{EmbeddingModel as FastEmbedModelName, TextEmbedding, TextInitOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use crate::error::MemoryError;
use crate::Result;

/// A chunk of text with its vector embedding and metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct MemoryChunk {
    pub id: String,
    pub text: String,
    #[serde(default = "Value::default")]
    pub metadata: Value,
    pub embedding: Vec<f32>,
}

impl MemoryChunk {
    pub fn new(id: impl Into<String>, text: impl Into<String>, metadata: Value) -> Self {
        MemoryChunk {
            id: id.into(),
            text: text.into(),
            metadata,
            embedding: Vec::new(),
        }
    }
}

#[async_trait]
pub trait EmbeddingModel: Send + Sync {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

#[derive(Clone)]
pub struct FastEmbedModel {
    inner: Arc<StdMutex<TextEmbedding>>,
}

impl FastEmbedModel {
    /// Construct a new FastEmbed model.
    ///
    /// This calls the synchronous `fastembed` initializer; call it from
    /// `tokio::task::spawn_blocking` if you are already inside an async context.
    pub fn try_new() -> Result<Self> {
        let cache_dir = default_cache_dir();
        let options =
            TextInitOptions::new(FastEmbedModelName::AllMiniLML6V2).with_cache_dir(cache_dir);
        let inner =
            TextEmbedding::try_new(options).map_err(|e| MemoryError::Embedding(e.to_string()))?;
        Ok(FastEmbedModel {
            inner: Arc::new(StdMutex::new(inner)),
        })
    }

    pub fn try_new_arc() -> Result<Arc<Self>> {
        Ok(Arc::new(Self::try_new()?))
    }
}

#[async_trait]
impl EmbeddingModel for FastEmbedModel {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let owned: Vec<String> = texts.iter().map(|s| (*s).to_string()).collect();
        let model = self.clone();
        tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
            model
                .inner
                .lock()
                .map_err(|e| MemoryError::Embedding(e.to_string()))?
                .embed(refs, None)
                .map_err(|e| MemoryError::Embedding(e.to_string()))
        })
        .await
        .map_err(|e| MemoryError::Embedding(e.to_string()))?
    }
}

fn default_cache_dir() -> PathBuf {
    cache_dir_for(
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".")),
    )
}

fn cache_dir_for(home: PathBuf) -> PathBuf {
    home.join(".cache").join("zerochain").join("models")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_chunk_new() {
        let chunk = MemoryChunk::new("id-1", "hello", serde_json::json!({"source": "x.md"}));
        assert_eq!(chunk.id, "id-1");
        assert_eq!(chunk.text, "hello");
        assert!(chunk.embedding.is_empty());
    }

    #[test]
    fn cache_dir_under_home() {
        let dir = cache_dir_for(PathBuf::from("/tmp/fakehome"));
        assert!(dir.ends_with(".cache/zerochain/models"));
    }
}
