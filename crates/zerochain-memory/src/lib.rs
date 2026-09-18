//! Filesystem-native vector memory and semantic search for zerochain.

pub mod chunk;
pub mod error;
pub mod graph;
pub mod graph_index;
pub mod graph_store;
pub mod model;
pub mod record;
pub mod similarity;
pub mod store;

pub use chunk::chunk_text;
pub use error::MemoryError;
pub use graph::Graph;
pub use graph_index::{GraphIndex, GraphView};
pub use graph_store::ContributionStore;
pub use model::{EmbeddingModel, FastEmbedModel, MemoryChunk};
pub use record::{
    ContributionMetric, ContributionRecord, ContributionType, MetricDirection, Verdict,
};
pub use similarity::cosine_similarity;
pub use store::MemoryStore;

/// Result type alias used throughout this crate.
pub type Result<T> = std::result::Result<T, MemoryError>;
