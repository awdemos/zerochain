//! Integration tests exercising the full zerochain lifecycle end-to-end.
//!
//! Uses the underlying library crates (zerochain-core, zerochain-fs, zerochain-cas)
//! directly since the daemon is a binary crate.

use std::path::Path;
use std::collections::HashMap;

use tempfile::TempDir;
use zerochain_core::task::{Task, TaskExecution};
use zerochain_core::workflow::Workflow;
use zerochain_core::context::Context;
use zerochain_fs::atomic::{
    acquire_lock, clean_output, is_complete, is_error, is_locked,
    mark_complete, mark_error, mark_executing,
};
use zerochain_cas::CasStore;
use zerochain_daemon::state::AppState;
use zerochain_llm::{
    CompleteResponse, LLM, LLMConfig, Message, ProviderId, Role,
};

fn make_task(id: &str, stages: Vec<&str>) -> Task {
    Task::new(
        id.to_string(),
        id.to_string(),
        "todo".to_string(),
        None,
        Some(TaskExecution::new(
            stages.into_iter().map(|s| s.to_string()).collect(),
            Some("sequential".to_string()),
        )),
        vec![],
        String::new(),
        None,
    )
}

fn workflow_dir(workspace: &Path) -> std::path::PathBuf {
    workspace.join(".zerochain").join("workflows")
}

async fn init_workflow(
    workspace: &Path,
    task: &Task,
) -> (Workflow, std::path::PathBuf) {
    let base = workflow_dir(workspace);
    tokio::fs::create_dir_all(&base).await.expect("create workflow base dir");
    let wf = Workflow::init(task, &base).await.expect("workflow init");
    let root = wf.root.clone();
    (wf, root)
}

async fn reload_workflow(root: &Path) -> Workflow {
    Workflow::from_dir(root).await.expect("reload workflow from disk")
}

// ---------------------------------------------------------------------------
// Test 1: workflow_init_creates_directory_structure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_init_creates_directory_structure() {
    let tmp = TempDir::new().expect("tempdir");
    let task = make_task("my-task", vec!["01_research", "02_design", "03_implement"]);
    let (wf, _root) = init_workflow(tmp.path(), &task).await;

    assert_eq!(wf.stages.len(), 3);

    let expected_stages = ["research", "design", "implement"];
    for name in &expected_stages {
        let stage = wf.stage_by_name(name)
            .unwrap_or_else(|| panic!("stage {name} not found"));
        assert!(stage.path.is_dir(), "stage dir {} missing", stage.path.display());
        assert!(stage.input_path.is_dir(), "input/ missing for {name}");
        assert!(stage.output_path.is_dir(), "output/ missing for {name}");
        assert!(stage.context_path.is_file(), "CONTEXT.md missing for {name}");
    }
}

// ---------------------------------------------------------------------------
// Test 2: workflow_init_parallel_stages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_init_parallel_stages() {
    let tmp = TempDir::new().expect("tempdir");
    let task = make_task("par-task", vec![
        "01_research",
        "02a_design",
        "02b_prototype",
        "03_review",
    ]);
    let (wf, _root) = init_workflow(tmp.path(), &task).await;

    assert_eq!(wf.stages.len(), 4);
    assert_eq!(wf.stages[0].id.raw, "01_research");
    assert_eq!(wf.stages[1].id.raw, "02a_design");
    assert_eq!(wf.stages[2].id.raw, "02b_prototype");
    assert_eq!(wf.stages[3].id.raw, "03_review");

    let plan = wf.execution_plan();
    assert_eq!(plan.groups.len(), 3);

    let parallel_group = &plan.groups[1];
    assert_eq!(parallel_group.stages.len(), 2);
    assert!(parallel_group.stages.iter().any(|s| s.raw == "02a_design"));
    assert!(parallel_group.stages.iter().any(|s| s.raw == "02b_prototype"));

    let review_node = plan.stage_map.get("03_review").expect("review node");
    assert!(review_node.dependencies.iter().any(|d| d.raw == "02a_design"));
    assert!(review_node.dependencies.iter().any(|d| d.raw == "02b_prototype"));
}

// ---------------------------------------------------------------------------
// Test 3: stage_lifecycle_complete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stage_lifecycle_complete() {
    let tmp = TempDir::new().expect("tempdir");
    let task = make_task("lifecycle", vec!["01_alpha", "02_beta", "03_gamma"]);
    let (wf, root) = init_workflow(tmp.path(), &task).await;

    let stage1 = wf.stage_by_name("alpha").expect("stage 1");
    mark_complete(&stage1.path, None).await.expect("mark complete");

    let reloaded = reload_workflow(&root).await;
    let reloaded_s1 = reloaded.stage_by_name("alpha").expect("reloaded stage 1");

    assert!(reloaded_s1.is_complete);
    assert!(!reloaded_s1.is_error);
    assert!(stage1.path.join(".complete").exists());
    assert!(!stage1.path.join(".error").exists());

    let plan = reloaded.execution_plan();
    let next = plan.next_stage().expect("should have a next stage");
    assert_eq!(next.raw, "02_beta");
}

// ---------------------------------------------------------------------------
// Test 4: stage_lifecycle_error_and_retry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stage_lifecycle_error_and_retry() {
    let tmp = TempDir::new().expect("tempdir");
    let task = make_task("err-retry", vec!["01_first", "02_second"]);
    let (wf, _root) = init_workflow(tmp.path(), &task).await;

    let stage1 = wf.stage_by_name("first").expect("stage 1");

    mark_error(&stage1.path, "test error").await.expect("mark error");

    assert!(is_error(&stage1.path).await);
    assert!(!is_complete(&stage1.path).await);
    let err_content = tokio::fs::read_to_string(stage1.path.join(".error"))
        .await
        .expect("read .error");
    assert_eq!(err_content, "test error");
    assert!(!stage1.path.join(".complete").exists());

    mark_complete(&stage1.path, None).await.expect("mark complete after error");

    assert!(is_complete(&stage1.path).await);
    assert!(!is_error(&stage1.path).await);
    assert!(!stage1.path.join(".error").exists());
}

// ---------------------------------------------------------------------------
// Test 5: execution_plan_ordering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execution_plan_ordering() {
    let tmp = TempDir::new().expect("tempdir");
    let task = make_task("ordering", vec!["01_research", "02_design", "03_implement", "04_review"]);
    let (wf, _root) = init_workflow(tmp.path(), &task).await;

    let mut plan = wf.execution_plan();
    assert!(!plan.is_complete());

    let next = plan.next_stage().expect("next 1").clone();
    assert_eq!(next.raw, "01_research");
    plan.mark_complete(&next);

    let next = plan.next_stage().expect("next 2").clone();
    assert_eq!(next.raw, "02_design");
    plan.mark_complete(&next);

    let next = plan.next_stage().expect("next 3").clone();
    assert_eq!(next.raw, "03_implement");
    plan.mark_complete(&next);

    let next = plan.next_stage().expect("next 4").clone();
    assert_eq!(next.raw, "04_review");
    plan.mark_complete(&next);

    assert!(plan.is_complete());
    assert!(plan.next_stage().is_none());
}

// ---------------------------------------------------------------------------
// Test 6: human_gate_blocks_execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn human_gate_blocks_execution() {
    let tmp = TempDir::new().expect("tempdir");
    let task = make_task("gate-test", vec!["01_build", "02_review"]);
    let (wf, root) = init_workflow(tmp.path(), &task).await;

    let stage2 = wf.stage_by_name("review").expect("stage 2");
    let ctx_content = "---\nhuman_gate: true\n---\n# Review stage\n";
    tokio::fs::write(&stage2.context_path, ctx_content)
        .await
        .expect("write context");

    let reloaded = reload_workflow(&root).await;
    let reloaded_s2 = reloaded.stage_by_name("review").expect("reloaded stage 2");
    assert!(reloaded_s2.human_gate);

    mark_complete(&reloaded_s2.path, None).await.expect("mark complete");
    assert!(is_complete(&reloaded_s2.path).await);
}

// ---------------------------------------------------------------------------
// Test 7: file_locking_prevents_concurrent_access
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_locking_prevents_concurrent_access() {
    let tmp = TempDir::new().expect("tempdir");
    let stage_dir = tmp.path().join("01_stage");
    tokio::fs::create_dir_all(&stage_dir).await.expect("mkdir");

    let guard = acquire_lock(&stage_dir).await.expect("first acquire");

    // is_locked checks if a DIFFERENT live process holds it; self-owned returns false
    assert!(!is_locked(&stage_dir).await);
    assert!(stage_dir.join(".lock").exists());

    drop(guard);
    assert!(!stage_dir.join(".lock").exists());
    assert!(!is_locked(&stage_dir).await);
}

// ---------------------------------------------------------------------------
// Test 8: file_locking_stale_detection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_locking_stale_detection() {
    let tmp = TempDir::new().expect("tempdir");
    let stage_dir = tmp.path().join("01_stage");
    tokio::fs::create_dir_all(&stage_dir).await.expect("mkdir");

    let stale_content = "PID:999999\nTIMESTAMP:0\n";
    tokio::fs::write(stage_dir.join(".lock"), stale_content)
        .await
        .expect("write stale lock");

    assert!(!is_locked(&stage_dir).await);
    let _guard = acquire_lock(&stage_dir).await.expect("acquire after stale");
}

// ---------------------------------------------------------------------------
// Test 9: output_cleanup_before_execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn output_cleanup_before_execution() {
    let tmp = TempDir::new().expect("tempdir");
    let stage_dir = tmp.path().join("01_stage");
    let output = stage_dir.join("output");
    tokio::fs::create_dir_all(&output).await.expect("mkdir output");
    tokio::fs::write(output.join("file1.txt"), b"data1")
        .await
        .expect("write file1");
    tokio::fs::write(output.join("file2.txt"), b"data2")
        .await
        .expect("write file2");

    clean_output(&stage_dir).await.expect("clean output");

    assert!(output.is_dir());
    let mut count = 0;
    let mut entries = tokio::fs::read_dir(&output).await.expect("read_dir");
    while entries.next_entry().await.expect("next").is_some() {
        count += 1;
    }
    assert_eq!(count, 0);
}

// ---------------------------------------------------------------------------
// Test 10: marker_mutual_exclusivity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn marker_mutual_exclusivity() {
    let tmp = TempDir::new().expect("tempdir");
    let stage_dir = tmp.path().join("01_stage");
    tokio::fs::create_dir_all(&stage_dir).await.expect("mkdir");

    mark_complete(&stage_dir, None).await.expect("mark complete");
    assert!(stage_dir.join(".complete").exists());
    assert!(!stage_dir.join(".error").exists());

    mark_error(&stage_dir, "fail").await.expect("mark error");
    assert!(stage_dir.join(".error").exists());
    assert!(!stage_dir.join(".complete").exists());

    // mark_executing does not remove .error (only mark_complete/mark_error do cleanup)
    mark_executing(&stage_dir).await.expect("mark executing");
    assert!(stage_dir.join(".executing").exists());
    assert!(stage_dir.join(".error").exists());

    mark_complete(&stage_dir, None).await.expect("mark complete final");
    assert!(stage_dir.join(".complete").exists());
    assert!(!stage_dir.join(".executing").exists());
    assert!(!stage_dir.join(".error").exists());
}

// ---------------------------------------------------------------------------
// Test 11: workflow_status_listing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_status_listing() {
    let tmp = TempDir::new().expect("tempdir");

    let task1 = make_task("alpha", vec!["01_build", "02_test"]);
    let task2 = make_task("beta", vec!["01_design", "02_code"]);
    let (wf1, root1) = init_workflow(tmp.path(), &task1).await;
    let (_wf2, _root2) = init_workflow(tmp.path(), &task2).await;

    let base = workflow_dir(tmp.path());
    let mut workflows: HashMap<String, Workflow> = HashMap::new();
    let mut entries = tokio::fs::read_dir(&base).await.expect("read_dir");
    while let Some(entry) = entries.next_entry().await.expect("next") {
        if entry.file_type().await.expect("filetype").is_dir() {
            if let Ok(wf) = Workflow::from_dir(&entry.path()).await {
                workflows.insert(wf.id.clone(), wf);
            }
        }
    }

    assert_eq!(workflows.len(), 2);

    for stage in &wf1.stages {
        mark_complete(&stage.path, None).await.expect("mark complete");
    }

    let alpha = Workflow::from_dir(&root1).await.expect("reload alpha");
    assert!(alpha.execution_plan().is_complete());

    let beta_entry = workflows.get("beta").expect("beta workflow");
    assert!(!beta_entry.execution_plan().is_complete());
}

// ---------------------------------------------------------------------------
// Test 12: context_inheritance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_inheritance() {
    let parent_ctx = Context::parse(
        "---\nrole: researcher\ntimeout: 60\n---\n# Stage 1\n",
    ).expect("parse parent");
    let child_ctx = Context::parse(
        "---\n---\n# Stage 2\n",
    ).expect("parse child");

    let merged = child_ctx.flatten(Some(&parent_ctx));
    assert_eq!(merged.frontmatter.role.as_deref(), Some("researcher"));
    assert_eq!(merged.frontmatter.timeout, Some(60));
    assert_eq!(merged.body, "# Stage 2\n");
}

// ---------------------------------------------------------------------------
// Test 13: cas_integration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cas_integration() {
    let tmp = TempDir::new().expect("tempdir");
    let store = CasStore::new(tmp.path().to_path_buf())
        .await
        .expect("create store");

    let data = b"integration test content";
    let cid = store.put(data).await.expect("put");
    let retrieved = store.get(&cid).await.expect("get");
    assert_eq!(&retrieved, data);

    let cid2 = store.put(data).await.expect("put again");
    assert_eq!(cid, cid2);
}

// ---------------------------------------------------------------------------
// Test 14: full_workflow_lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_workflow_lifecycle() {
    let tmp = TempDir::new().expect("tempdir");
    let task = make_task("e2e-task", vec!["01_setup", "02_build", "03_verify"]);
    let (wf, root) = init_workflow(tmp.path(), &task).await;

    assert_eq!(wf.stages.len(), 3);

    for stage in &wf.stages {
        let guard = acquire_lock(&stage.path).await.expect("acquire lock");
        clean_output(&stage.path).await.expect("clean output");
        mark_complete(&stage.path, None).await.expect("mark complete");
        drop(guard);
    }

    let final_wf = reload_workflow(&root).await;

    for stage in &final_wf.stages {
        assert!(stage.is_complete, "stage {} not complete", stage.id.raw);
        assert!(!stage.is_error, "stage {} has error", stage.id.raw);
        assert!(stage.path.join(".complete").exists());
    }

    assert!(final_wf.execution_plan().is_complete());

    for stage in &final_wf.stages {
        assert!(!stage.path.join(".lock").exists(), "stale .lock in {}", stage.id.raw);
        assert!(!stage.path.join(".executing").exists(), "stale .executing in {}", stage.id.raw);
    }
}

// ---------------------------------------------------------------------------
// Test 15: execute_stage_with_mock_llm
// ---------------------------------------------------------------------------

struct MockLLM {
    response_content: String,
}

impl MockLLM {
    fn new(content: impl Into<String>) -> Self {
        Self { response_content: content.into() }
    }
}

#[async_trait::async_trait]
impl LLM for MockLLM {
    fn provider_id(&self) -> &ProviderId {
        static ID: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
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
                zerochain_llm::Content::Text(s) => s.clone(),
            })
            .unwrap_or_default();

        let content = if user_input.contains("echo") {
            user_input
        } else if !user_input.is_empty() {
            format!("MOCK RECEIVED: {}", user_input)
        } else {
            self.response_content.clone()
        };

        Ok(CompleteResponse::new(content))
    }

    fn supports_multimodal(&self) -> bool { false }
    fn context_window(&self) -> usize { 128_000 }
    async fn health_check(&self) -> Result<(), zerochain_llm::LLMError> { Ok(()) }
}

#[tokio::test]
async fn execute_stage_writes_result_from_llm() {
    let tmp = TempDir::new().expect("tempdir");
    let task = make_task("llm-test", vec!["01_analyze", "02_synthesize"]);
    let (wf, _root) = init_workflow(tmp.path(), &task).await;

    let stage1 = wf.stage_by_name("analyze").expect("stage 1");

    let state = AppState::new(tmp.path());
    let mock = MockLLM::new("Mock analysis result from LLM.");
    state
        .execute_stage_with_llm("llm-test", stage1, &mock)
        .await
        .expect("execute stage");

    let result_path = stage1.output_path.join("result.md");
    assert!(result_path.exists(), "result.md should be written");
    let content = tokio::fs::read_to_string(&result_path)
        .await
        .expect("read result.md");
    assert_eq!(content, "Mock analysis result from LLM.");
}

#[tokio::test]
async fn execute_stage_passes_context_to_llm() {
    let tmp = TempDir::new().expect("tempdir");
    let task = make_task("ctx-test", vec!["01_review"]);
    let (wf, _root) = init_workflow(tmp.path(), &task).await;

    let stage = wf.stage_by_name("review").expect("stage 1");
    tokio::fs::write(
        &stage.context_path,
        "---\nrole: code reviewer\n---\nReview the code for bugs.\n",
    )
    .await
    .expect("write context");

    tokio::fs::write(stage.input_path.join("previous.md"), "Here is the prior output.")
        .await
        .expect("write input");

    let echo_mock = MockLLM::new(String::new());
    let state = AppState::new(tmp.path());
    state
        .execute_stage_with_llm("ctx-test", stage, &echo_mock)
        .await
        .expect("execute stage");

    let result_path = stage.output_path.join("result.md");
    let content = tokio::fs::read_to_string(&result_path)
        .await
        .expect("read result");
    assert!(content.contains("Here is the prior output."), "should pass input files to LLM");
}

#[tokio::test]
async fn execute_stage_handles_missing_context_gracefully() {
    let tmp = TempDir::new().expect("tempdir");
    let task = make_task("no-ctx", vec!["01_step"]);
    let (wf, _root) = init_workflow(tmp.path(), &task).await;

    let stage = wf.stage_by_name("step").expect("stage 1");
    tokio::fs::remove_file(&stage.context_path).await.expect("remove context");

    let mock = MockLLM::new("No context needed.");
    let state = AppState::new(tmp.path());
    state
        .execute_stage_with_llm("no-ctx", stage, &mock)
        .await
        .expect("execute without context");

    let content = tokio::fs::read_to_string(stage.output_path.join("result.md"))
        .await
        .expect("read result");
    assert_eq!(content, "No context needed.");
}
