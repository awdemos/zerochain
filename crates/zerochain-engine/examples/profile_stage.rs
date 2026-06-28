use std::sync::OnceLock;

use async_trait::async_trait;
use tokio::runtime::Runtime;
use tracing_subscriber::fmt::format::FmtSpan;

use zerochain_core::task::{Task, TaskExecution};
use zerochain_core::workflow::Workflow;
use zerochain_engine::AppState;
use zerochain_llm::{CompleteResponse, LLMConfig, Message, ProviderId, Role, LLM};

struct MockLLM;

#[async_trait]
impl LLM for MockLLM {
    fn provider_id(&self) -> &ProviderId {
        static ID: OnceLock<ProviderId> = OnceLock::new();
        ID.get_or_init(|| ProviderId::OpenAI)
    }

    async fn complete(
        &self,
        _config: &LLMConfig,
        messages: &[Message],
        _tools: Option<&[zerochain_llm::Tool]>,
    ) -> Result<CompleteResponse, zerochain_llm::LLMError> {
        let user_input = messages
            .iter()
            .find(|m| matches!(m.role, Role::User))
            .and_then(|m| m.content.text())
            .unwrap_or("")
            .to_string();
        let content = if !user_input.is_empty() {
            format!("MOCK RECEIVED: {user_input}")
        } else {
            "mock response".into()
        };
        Ok(CompleteResponse::new(content))
    }

    fn supports_multimodal(&self) -> bool {
        false
    }
    fn context_window(&self) -> usize {
        128_000
    }
    async fn health_check(&self) -> Result<(), zerochain_llm::LLMError> {
        Ok(())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .with_target(false)
        .with_span_events(FmtSpan::CLOSE)
        .init();

    let tmp = tempfile::tempdir().expect("tempdir");
    let rt = Runtime::new().expect("tokio runtime");
    let wf_base = tmp.path().join(".zerochain").join("workflows");
    std::fs::create_dir_all(&wf_base).expect("create workflows dir");

    let task = Task::builder("profile-wf", "profile-wf")
        .execution(TaskExecution::new(
            vec!["01_analyze".into(), "02_synthesize".into()],
            Some("sequential".into()),
        ))
        .build();

    let wf = rt
        .block_on(Workflow::init(&task, &wf_base))
        .expect("init workflow");
    let stage = wf.stage_by_name("analyze").expect("analyze stage");

    std::fs::write(stage.input_path.join("data.md"), "profile input data").expect("write input");

    rt.block_on(async {
        let mut state = AppState::new(tmp.path(), None).await;
        state
            .execute_stage_with_llm("profile-wf", stage, &MockLLM)
            .await
            .expect("execute stage");
    });

    let result_path = stage.output_path.join("result.md");
    assert!(result_path.exists(), "result.md should exist");
    println!("Profile run complete. Check logs above for per-phase timings.");
}
