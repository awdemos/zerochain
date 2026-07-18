//! Filesystem-native vector memory and semantic search for zerochain.

pub mod chunk;
pub mod error;
pub mod model;
pub mod similarity;
pub mod store;

pub use chunk::chunk_text;
pub use error::MemoryError;
pub use model::{EmbeddingModel, FastEmbedModel, MemoryChunk};
pub use similarity::cosine_similarity;
pub use store::MemoryStore;

/// Result type alias used throughout this crate.
pub type Result<T> = std::result::Result<T, MemoryError>;
