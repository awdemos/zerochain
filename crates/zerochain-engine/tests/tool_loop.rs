use async_trait::async_trait;
use std::sync::Mutex;
use tempfile::TempDir;
use zerochain_engine::state::{AppState, InitWorkflowParams};
use zerochain_llm::{
    CompleteResponse, FinishReason, LLMConfig, Message, ProviderId, Role, Tool, ToolCall, LLM,
};

struct MockLoopLlm {
    calls: Mutex<Vec<Vec<Message>>>,
}

#[async_trait]
impl LLM for MockLoopLlm {
    fn provider_id(&self) -> &ProviderId {
        &ProviderId::OpenAI
    }

    async fn complete(
        &self,
        _config: &LLMConfig,
        messages: &[Message],
        tools: Option<&[Tool]>,
    ) -> Result<CompleteResponse, zerochain_llm::error::LLMError> {
        let tools = tools.expect("tools should be passed");
        assert!(tools.iter().any(|t| t.name == "read_file"));

        let mut calls = self.calls.lock().unwrap();
        calls.push(messages.to_vec());
        let call_count = calls.len();
        drop(calls);

        if call_count == 1 {
            let mut response = CompleteResponse::new(None);
            response.tool_calls = vec![ToolCall::new(
                "call_1",
                "read_file",
                serde_json::json!({ "path": "loop.txt" }),
            )];
            response.finish_reason = FinishReason::ToolCalls;
            response.model = "mock".into();
            Ok(response)
        } else {
            Ok(CompleteResponse::new(Some("done with tool result".into())))
        }
    }

    fn supports_multimodal(&self) -> bool {
        false
    }

    fn context_window(&self) -> usize {
        4096
    }

    async fn health_check(&self) -> Result<(), zerochain_llm::error::LLMError> {
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[tokio::test]
async fn tool_loop_feeds_result_back_to_llm() {
    let tmp = TempDir::new().unwrap();
    let mut state = AppState::new(tmp.path(), None).await;

    let wf = state
        .init_workflow(InitWorkflowParams {
            name: "loop-wf",
            path: None,
            template: Some("00_loop"),
            force: false,
        })
        .await
        .unwrap();
    let stage = wf.stages[0].clone();

    tokio::fs::write(
        &stage.context_path,
        "---\nrole: loop tester\ntools:\n  - read_file\n---\nLoop test.\n",
    )
    .await
    .unwrap();
    tokio::fs::write(wf.root.join("loop.txt"), "mock-file-content")
        .await
        .unwrap();

    let llm = MockLoopLlm {
        calls: Mutex::new(Vec::new()),
    };
    state
        .execute_stage_with_llm("loop-wf", &stage, &llm)
        .await
        .unwrap();

    let has_tool_result = {
        let calls = llm.calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "expected two LLM calls");

        let second_messages = &calls[1];
        second_messages.iter().any(|m| {
            matches!(m.role, Role::Tool)
                && m.content
                    .text()
                    .is_some_and(|t| t.contains("mock-file-content"))
        })
    };
    assert!(
        has_tool_result,
        "second LLM call should see the tool result"
    );

    let result = tokio::fs::read_to_string(stage.output_path.join("result.md"))
        .await
        .unwrap();
    assert_eq!(result.trim(), "done with tool result");
}
