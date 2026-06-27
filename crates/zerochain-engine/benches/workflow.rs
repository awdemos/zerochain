use std::path::Path;
use std::sync::OnceLock;

use async_trait::async_trait;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use tempfile::TempDir;
use tokio::runtime::Runtime;

use zerochain_core::task::{Task, TaskExecution};
use zerochain_core::workflow::Workflow;
use zerochain_engine::AppState;
use zerochain_llm::{CompleteResponse, Content, LLMConfig, Message, ProviderId, Role, LLM};

struct MockLLM {
    response: String,
}

impl MockLLM {
    fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
        }
    }
}

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
            .map(|m| match &m.content {
                Content::Text(s) => s.clone(),
                _ => String::new(),
            })
            .unwrap_or_default();

        let content = if !user_input.is_empty() {
            format!("MOCK RECEIVED: {user_input}")
        } else {
            self.response.clone()
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

fn init_workflow(workspace: &Path) -> (Workflow, String) {
    let rt = Runtime::new().expect("tokio runtime");
    let wf_base = workspace.join(".zerochain").join("workflows");
    std::fs::create_dir_all(&wf_base).expect("create workflows dir");

    let task = Task::builder("bench-wf", "bench-wf")
        .execution(TaskExecution::new(
            vec!["01_analyze".into(), "02_synthesize".into()],
            Some("sequential".into()),
        ))
        .build();

    let wf = rt
        .block_on(Workflow::init(&task, &wf_base))
        .expect("init workflow");
    (wf, wf_base.to_string_lossy().to_string())
}

fn criterion_benchmark(c: &mut Criterion) {
    let tmp = TempDir::new().expect("tempdir");
    let (wf, _wf_base) = init_workflow(tmp.path());
    let stage = wf.stage_by_name("analyze").expect("analyze stage");

    std::fs::write(stage.input_path.join("data.md"), "benchmark input data").expect("write input");

    let rt = Runtime::new().expect("tokio runtime");
    let mock = MockLLM::new("benchmark response");

    c.bench_function("execute_stage_with_llm", |b| {
        b.to_async(&rt).iter(|| async {
            let mut state = AppState::new(tmp.path(), None).await;
            let _ = state
                .execute_stage_with_llm("bench-wf", stage, &mock)
                .await
                .expect("execute stage");
            black_box(state);
        });
    });

    // Sanity check that the benchmark actually produced output.
    let result_path = stage.output_path.join("result.md");
    assert!(
        result_path.exists(),
        "result.md should exist after benchmark"
    );
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
