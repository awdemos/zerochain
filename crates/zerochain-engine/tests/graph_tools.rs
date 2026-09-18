use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;
use zerochain_engine::state::{AppState, InitWorkflowParams};
use zerochain_llm::{
    CompleteResponse, FinishReason, LLMConfig, Message, ProviderId, Tool, ToolCall, LLM,
};
use zerochain_memory::{ContributionType, Graph};

struct FakeEmbed;

#[async_trait]
impl zerochain_memory::EmbeddingModel for FakeEmbed {
    async fn embed(
        &self,
        texts: &[&str],
    ) -> std::result::Result<Vec<Vec<f32>>, zerochain_memory::MemoryError> {
        Ok(texts.iter().map(|_| vec![1.0f32, 0.0, 0.0]).collect())
    }
}

struct GraphToolLlm {
    calls: AtomicUsize,
}

fn last_json(messages: &[Message]) -> Value {
    let text = messages
        .last()
        .and_then(|m| m.content.text())
        .unwrap_or_default();
    let start = text.find('{').expect("tool result JSON in message");
    serde_json::from_str(&text[start..]).expect("valid tool result JSON")
}

#[async_trait]
impl LLM for GraphToolLlm {
    fn provider_id(&self) -> &ProviderId {
        static PROVIDER: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        PROVIDER.get_or_init(|| ProviderId::OpenAI)
    }

    async fn complete(
        &self,
        _config: &LLMConfig,
        messages: &[Message],
        tools: Option<&[Tool]>,
    ) -> std::result::Result<CompleteResponse, zerochain_llm::error::LLMError> {
        let tools = tools.expect("tools should be passed to the LLM");
        assert!(tools.iter().any(|t| t.name == "contribute"));
        assert!(tools.iter().any(|t| t.name == "verify"));
        assert!(tools.iter().any(|t| t.name == "graph_query"));

        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut response = CompleteResponse::new(None);
        response.model = "mock".into();

        match call {
            0 => {
                response.tool_calls = vec![ToolCall::new(
                    "call_1",
                    "contribute",
                    json!({
                        "type": "insight",
                        "body": "donor ensembling improves transfer"
                    }),
                )];
                response.finish_reason = FinishReason::ToolCalls;
            }
            1 => {
                let id = last_json(messages)
                    .get("id")
                    .and_then(Value::as_str)
                    .expect("contribute returns an id")
                    .to_string();
                response.tool_calls = vec![ToolCall::new(
                    "call_2",
                    "verify",
                    json!({
                        "target": id,
                        "verdict": "confirmed",
                        "body": "reproduced on a second run"
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
async fn graph_tools_publish_and_verify_through_tool_loop() {
    let tmp = TempDir::new().unwrap();
    let mut state = AppState::new(tmp.path(), None).await;
    state.embedding_model = Some(Arc::new(FakeEmbed));

    let wf = state
        .init_workflow(InitWorkflowParams {
            name: "graph-tools-wf",
            path: None,
            template: Some("00_spec"),
            force: false,
            parents: Vec::new(),
        })
        .await
        .unwrap();

    let stage = wf.stages[0].clone();
    tokio::fs::write(
        &stage.context_path,
        "---\nrole: graph tool runner\ntools:\n  - contribute\n  - verify\n  - graph_query\n---\nUse the graph tools.\n",
    )
    .await
    .unwrap();

    let llm = GraphToolLlm {
        calls: AtomicUsize::new(0),
    };
    state
        .execute_stage_with_llm("graph-tools-wf", &stage, &llm)
        .await
        .unwrap();

    let result = tokio::fs::read_to_string(stage.output_path.join("result.md"))
        .await
        .unwrap();
    assert!(
        result.contains("\"id\""),
        "final output carries tool result; got: {result}"
    );

    let graph = Graph::open(tmp.path().join(".zerochain").join("graph"))
        .await
        .unwrap();
    let verifications: Vec<_> = graph
        .index()
        .all()
        .into_iter()
        .filter(|r| r.record_type == ContributionType::Verification)
        .cloned()
        .collect();
    assert_eq!(verifications.len(), 1, "one verification published");
    let insights: Vec<_> = graph
        .index()
        .all()
        .into_iter()
        .filter(|r| r.record_type == ContributionType::Insight)
        .cloned()
        .collect();
    assert_eq!(insights.len(), 1);
    assert_eq!(verifications[0].target, Some(insights[0].id.clone()));
    assert_eq!(verifications[0].stage.as_deref(), Some("00_spec"));
    assert!(
        insights[0].actor.starts_with("zerochain/"),
        "actor injected: {}",
        insights[0].actor
    );
    assert_eq!(
        insights[0].workflow.as_deref(),
        Some("graph-tools-wf"),
        "workflow context injected"
    );
}
