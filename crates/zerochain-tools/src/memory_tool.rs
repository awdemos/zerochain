use async_trait::async_trait;
use serde_json::{json, Value};
use zerochain_error::{Result, ZerochainError};
use zerochain_memory::{chunk_text, MemoryStore};

use crate::tool::Tool;

const DEFAULT_CHUNK_SIZE: usize = 1000;
const DEFAULT_CHUNK_OVERLAP: usize = 200;

/// Tool that stores text chunks into the workflow memory store.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryStoreTool;

#[async_trait]
impl Tool for MemoryStoreTool {
    fn name(&self) -> &str {
        "memory_store"
    }

    fn description(&self) -> &str {
        "Store text chunks into the workflow memory store for later semantic search."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "texts": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "description": "Text to store." },
                            "metadata": { "type": "object", "description": "Optional metadata." },
                            "chunk_size": { "type": "number", "description": "Optional chunk size (default 1000)." },
                            "chunk_overlap": { "type": "number", "description": "Optional chunk overlap (default 200)." }
                        },
                        "required": ["text"]
                    },
                    "description": "List of text entries to chunk and store."
                },
                "memory_store_path": {
                    "type": "string",
                    "description": "Injected by the engine; do not set manually."
                }
            },
            "required": ["texts"]
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let texts = input
            .get("texts")
            .and_then(Value::as_array)
            .ok_or_else(|| ZerochainError::InvalidInput {
                message: "missing 'texts' array".to_string(),
            })?;

        let path = input
            .get("memory_store_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ZerochainError::InvalidInput {
                message: "missing 'memory_store_path'".to_string(),
            })?;

        let mut store = MemoryStore::open(path).await?;

        let mut chunks: Vec<(String, Value)> = Vec::new();
        for entry in texts {
            let text = entry.get("text").and_then(Value::as_str).ok_or_else(|| {
                ZerochainError::InvalidInput {
                    message: "each text entry requires a 'text' field".to_string(),
                }
            })?;
            let metadata = entry.get("metadata").cloned().unwrap_or_else(|| json!({}));
            let chunk_size = entry
                .get("chunk_size")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(DEFAULT_CHUNK_SIZE);
            let chunk_overlap = entry
                .get("chunk_overlap")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(DEFAULT_CHUNK_OVERLAP);

            for chunk in chunk_text(text, chunk_size, chunk_overlap) {
                chunks.push((chunk, metadata.clone()));
            }
        }

        let model =
            zerochain_memory::FastEmbedModel::try_new().map_err(|e| ZerochainError::Other {
                message: format!("failed to initialize embedding model: {e}"),
            })?;

        let stored = store.add(&model, chunks).await?;
        Ok(json!({ "stored": stored }))
    }
}

/// Tool that queries the workflow memory store by semantic similarity.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryQueryTool;

#[async_trait]
impl Tool for MemoryQueryTool {
    fn name(&self) -> &str {
        "memory_query"
    }

    fn description(&self) -> &str {
        "Query the workflow memory store for chunks semantically similar to a query."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Query text." },
                "top_k": { "type": "number", "description": "Number of results to return (default 5)." },
                "memory_store_path": {
                    "type": "string",
                    "description": "Injected by the engine; do not set manually."
                }
            },
            "required": ["query"]
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let query = input.get("query").and_then(Value::as_str).ok_or_else(|| {
            ZerochainError::InvalidInput {
                message: "missing 'query' field".to_string(),
            }
        })?;

        let top_k = input
            .get("top_k")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(5);

        let path = input
            .get("memory_store_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ZerochainError::InvalidInput {
                message: "missing 'memory_store_path'".to_string(),
            })?;

        let store = MemoryStore::open(path).await?;
        let model =
            zerochain_memory::FastEmbedModel::try_new().map_err(|e| ZerochainError::Other {
                message: format!("failed to initialize embedding model: {e}"),
            })?;

        let results = store.query(&model, query, top_k).await?;
        let json_results: Vec<Value> = results
            .into_iter()
            .map(|(score, chunk)| {
                json!({
                    "text": chunk.text,
                    "score": score,
                    "metadata": chunk.metadata,
                })
            })
            .collect();

        Ok(json!({ "results": json_results }))
    }
}
