use async_trait::async_trait;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use tempfile::TempDir;
use zerochain_engine::state::{AppState, InitWorkflowParams};
use zerochain_llm::{
    CompleteResponse, FinishReason, LLMConfig, Message, ProviderId, Tool, ToolCall, LLM,
};
use zerochain_memory::{EmbeddingModel, MemoryError};

struct FakeEmbed;

#[async_trait]
impl EmbeddingModel for FakeEmbed {
    async fn embed(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, MemoryError> {
        Ok(texts.iter().map(|_| vec![1.0f32, 0.0, 0.0]).collect())
    }
}

struct MemoryToolLlm {
    calls: AtomicUsize,
}

#[async_trait]
impl LLM for MemoryToolLlm {
    fn provider_id(&self) -> &ProviderId {
        static PROVIDER: OnceLock<ProviderId> = OnceLock::new();
        PROVIDER.get_or_init(|| ProviderId::OpenAI)
    }

    async fn complete(
        &self,
        _config: &LLMConfig,
        messages: &[Message],
        tools: Option<&[Tool]>,
    ) -> std::result::Result<CompleteResponse, zerochain_llm::error::LLMError> {
        let tools = tools.expect("tools should be passed to the LLM");
        assert!(tools.iter().any(|t| t.name == "memory_store"));
        assert!(tools.iter().any(|t| t.name == "memory_query"));

        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut response = CompleteResponse::new(None);
        response.model = "mock".into();

        match call {
            0 => {
                response.tool_calls = vec![ToolCall::new(
                    "call_1",
                    "memory_store",
                    json!({
                        "texts": [{
                            "text": "zerochain semantic search memory",
                            "metadata": {"source": "test"}
                        }]
                    }),
                )];
                response.finish_reason = FinishReason::ToolCalls;
            }
            1 => {
                response.tool_calls = vec![ToolCall::new(
                    "call_2",
                    "memory_query",
                    json!({
                        "query": "semantic search",
                        "top_k": 5
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
async fn memory_tools_can_query_indexed_output() {
    let tmp = TempDir::new().unwrap();
    let mut state = AppState::new(tmp.path(), None).await;
    state.embedding_model = Some(Arc::new(FakeEmbed));

    let wf = state
        .init_workflow(InitWorkflowParams {
            name: "mem-tools-wf",
            path: None,
            template: Some("00_memory"),
            force: false,
        })
        .await
        .unwrap();

    let stage = wf.stages[0].clone();
    tokio::fs::write(
        &stage.context_path,
        "---\nrole: memory tool runner\ntools:\n  - memory_store\n  - memory_query\n---\nUse the memory tools.\n",
    )
    .await
    .unwrap();

    let llm = MemoryToolLlm {
        calls: AtomicUsize::new(0),
    };
    state
        .execute_stage_with_llm("mem-tools-wf", &stage, &llm)
        .await
        .unwrap();

    let result = tokio::fs::read_to_string(stage.output_path.join("result.md"))
        .await
        .unwrap();
    assert!(
        result.contains("results"),
        "query should return results; got: {result}"
    );
    assert!(
        result.contains("zerochain semantic search memory"),
        "result should contain the stored chunk; got: {result}"
    );
}
