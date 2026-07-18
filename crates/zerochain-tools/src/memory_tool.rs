use async_trait::async_trait;
use serde_json::{json, Value};
use zerochain_error::{Result, ZerochainError};
use zerochain_memory::{chunk_text, MemoryStore};

use crate::tool::Tool;

const DEFAULT_CHUNK_SIZE: usize = 1000;
const DEFAULT_CHUNK_OVERLAP: usize = 200;
const DEFAULT_TOP_K: usize = 5;

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

        let model = tokio::task::spawn_blocking(zerochain_memory::FastEmbedModel::try_new)
            .await
            .map_err(|e| ZerochainError::Other {
                message: e.to_string(),
            })?
            .map_err(|e| ZerochainError::Other {
                message: format!("failed to initialize embedding model: {e}"),
            })?;

        let mut store = MemoryStore::open(path).await?;
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
            .unwrap_or(DEFAULT_TOP_K);

        let path = input
            .get("memory_store_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ZerochainError::InvalidInput {
                message: "missing 'memory_store_path'".to_string(),
            })?;

        let store = MemoryStore::open(path).await?;
        let model = tokio::task::spawn_blocking(zerochain_memory::FastEmbedModel::try_new)
            .await
            .map_err(|e| ZerochainError::Other {
                message: e.to_string(),
            })?
            .map_err(|e| ZerochainError::Other {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use zerochain_memory::{FastEmbedModel, MemoryStore};

    #[tokio::test]
    async fn query_tool_returns_top_k_results_with_metadata() {
        let tmp = TempDir::new().unwrap();
        let model = FastEmbedModel::try_new().expect("failed to load embedding model");
        let mut store = MemoryStore::open(tmp.path()).await.unwrap();
        store
            .add(
                &model,
                vec![
                    (
                        "zerochain vector memory search".into(),
                        json!({"source": "a.md"}),
                    ),
                    (
                        "semantic embeddings in rust".into(),
                        json!({"source": "b.md"}),
                    ),
                    ("workflow memory store".into(), json!({"source": "c.md"})),
                ],
            )
            .await
            .unwrap();

        let tool = MemoryQueryTool;
        let input = json!({
            "query": "vector memory",
            "top_k": 2,
            "memory_store_path": tmp.path().to_str().unwrap(),
        });
        let result = tool.run(input).await.unwrap();
        let results = result
            .get("results")
            .and_then(|v| v.as_array())
            .expect("results array");
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| {
            r.get("metadata")
                .and_then(|m| m.get("source"))
                .and_then(|s| s.as_str())
                == Some("a.md")
        }));
    }

    #[tokio::test]
    async fn store_tool_rejects_missing_memory_store_path() {
        let tool = MemoryStoreTool;
        let input = json!({
            "texts": [{"text": "some text"}]
        });
        let err = tool.run(input).await.unwrap_err();
        assert!(err.to_string().contains("memory_store_path"));
    }
}
