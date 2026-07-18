//! Integration test for the research-v2 multi-agent template.

use tempfile::TempDir;
use zerochain_engine::{AppState, InitWorkflowParams};

#[tokio::test]
async fn init_research_v2_creates_four_stage_workflow() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::new(tmp.path(), None).await;

    let wf = state
        .init_workflow(InitWorkflowParams {
            name: "foo",
            path: None,
            template: Some("research-v2"),
            force: false,
        })
        .await
        .expect("init workflow with research-v2 template");

    assert_eq!(wf.stages.len(), 4, "research-v2 should have 4 stages");

    let expected_stages = ["00_ingest", "01_summarize", "02_critique", "03_runbook"];
    for (stage, expected_name) in wf.stages.iter().zip(expected_stages.iter()) {
        assert_eq!(
            &stage.id.raw, *expected_name,
            "unexpected stage name: {}",
            stage.id.raw
        );
        assert!(stage.path.is_dir(), "stage dir missing: {expected_name}");
        assert!(
            stage.input_path.is_dir(),
            "input/ missing for {expected_name}"
        );
        assert!(
            stage.output_path.is_dir(),
            "output/ missing for {expected_name}"
        );
        assert!(
            stage.context_path.is_file(),
            "CONTEXT.md missing for {expected_name}"
        );
    }

    assert!(
        state.get_workflow("foo").is_some(),
        "workflow should be registered"
    );
}

#[tokio::test]
async fn research_v2_contexts_reference_previous_stage() {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::new(tmp.path(), None).await;

    let wf = state
        .init_workflow(InitWorkflowParams {
            name: "bar",
            path: None,
            template: Some("research-v2"),
            force: false,
        })
        .await
        .expect("init workflow with research-v2 template");

    for stage in &wf.stages {
        let content = tokio::fs::read_to_string(&stage.context_path)
            .await
            .expect("read CONTEXT.md");
        assert!(
            content.contains("role:"),
            "CONTEXT.md should have a role frontmatter: {}",
            stage.id.raw
        );
    }

    let critique = wf.stage_by_name("critique").expect("critique stage");
    let critique_ctx = tokio::fs::read_to_string(&critique.context_path)
        .await
        .expect("read critique CONTEXT.md");
    assert!(critique_ctx.contains("zerochain.control.v1.return"));
    assert!(critique_ctx.contains("zerochain.control.v1.escalate"));
}
