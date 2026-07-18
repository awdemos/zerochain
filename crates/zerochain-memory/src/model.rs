//! Embedding model abstraction for vector memory.

/// Async trait for embedding models.
#[async_trait::async_trait]
pub trait EmbeddingModel {
    /// Embed a single chunk of text into a vector.
    async fn embed(&self, text: &str) -> crate::Result<Vec<f32>>;
}

/// FastEmbed-backed embedding model.
pub struct FastEmbedModel;

impl FastEmbedModel {
    /// Create a new FastEmbed model instance.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl EmbeddingModel for FastEmbedModel {
    async fn embed(&self, _text: &str) -> crate::Result<Vec<f32>> {
        Ok(Vec::new())
    }
}

/// A chunk of text and its associated embedding.
#[derive(Debug, Clone)]
pub struct MemoryChunk {
    /// The original text content.
    pub text: String,
    /// The vector embedding of the text.
    pub embedding: Vec<f32>,
}

impl MemoryChunk {
    /// Create a new memory chunk without an embedding.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            embedding: Vec::new(),
        }
    }
}

impl Default for FastEmbedModel {
    fn default() -> Self {
        Self::new()
    }
}
