use async_trait::async_trait;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zerochain_engine::state::{AppState, InitWorkflowParams};
use zerochain_llm::{
    CompleteResponse, FinishReason, LLMConfig, Message, ProviderId, Tool, ToolCall, LLM,
};

struct MockToolLlm {
    calls: std::sync::atomic::AtomicUsize,
    tool_response: CompleteResponse,
    final_response: CompleteResponse,
}

#[async_trait]
impl LLM for MockToolLlm {
    fn provider_id(&self) -> &ProviderId {
        &ProviderId::OpenAI
    }

    async fn complete(
        &self,
        _config: &LLMConfig,
        _messages: &[Message],
        tools: Option<&[Tool]>,
    ) -> Result<CompleteResponse, zerochain_llm::error::LLMError> {
        let tools = tools.expect("tools should be passed to the LLM");
        assert!(!tools.is_empty(), "tools list should not be empty");
        assert!(tools.iter().any(|t| t.name == "http"), "http tool missing");

        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call == 0 {
            Ok(self.tool_response.clone())
        } else {
            Ok(self.final_response.clone())
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
async fn tool_stage_executes_http_call() {
    let tmp = TempDir::new().unwrap();
    let mut state = AppState::new(tmp.path(), None).await;

    let wf = state
        .init_workflow(InitWorkflowParams {
            name: "tool-wf",
            path: None,
            template: Some("00_tool"),
            force: false,
        })
        .await
        .unwrap();

    let stage = wf.stages[0].clone();
    tokio::fs::write(
        &stage.context_path,
        "---\nrole: tool runner\ntools:\n  - http\n---\nRun the http tool.\n",
    )
    .await
    .unwrap();

    // Start a minimal TCP server that replies with a fixed JSON body.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let n = socket.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);
        assert!(
            request.starts_with("GET /mock"),
            "unexpected request: {request}"
        );

        let body = r#"{"tool_response":"mock-payload"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });

    let arguments = serde_json::json!({
        "url": format!("http://{}/mock", addr),
        "method": "GET"
    });
    let mut tool_response = CompleteResponse::new(None);
    tool_response.tool_calls = vec![ToolCall::new("call_1", "http", arguments)];
    tool_response.finish_reason = FinishReason::ToolCalls;
    tool_response.model = "mock".into();

    let mut final_response = CompleteResponse::new(Some("Final answer: mock-payload".into()));
    final_response.model = "mock".into();

    let llm = MockToolLlm {
        calls: std::sync::atomic::AtomicUsize::new(0),
        tool_response,
        final_response,
    };
    state
        .execute_stage_with_llm("tool-wf", &stage, &llm)
        .await
        .unwrap();

    let result = tokio::fs::read_to_string(stage.output_path.join("result.md"))
        .await
        .unwrap();
    assert!(
        result.contains("mock-payload"),
        "stage output should contain the mock server response; got: {result}"
    );

    server.await.unwrap();
}
