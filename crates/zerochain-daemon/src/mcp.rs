use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::ServerHandler;
use rmcp::model::Content;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;

use zerochain_core::workflow::is_valid_workflow_name;

use crate::{state::AppState, DaemonError};

pub struct ZerochainMcpServer {
    #[allow(dead_code)] // read by #[tool_router] macro-generated code
    tool_router: ToolRouter<Self>,
    state: Arc<Mutex<AppState>>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct InitParams {
    pub name: String,
    #[serde(default)]
    pub template: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RunParams {
    pub workflow_id: String,
    #[serde(default)]
    pub stage: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StatusParams {
    #[serde(default)]
    pub workflow_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct StageParams {
    pub workflow_id: String,
    pub stage_id: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct RejectParams {
    pub workflow_id: String,
    pub stage_id: String,
    #[serde(default)]
    pub feedback: Option<String>,
}

impl ZerochainMcpServer {
    pub fn new(workspace: PathBuf) -> Self {
        let state = AppState::new(&workspace);
        Self {
            tool_router: Self::tool_router(),
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub async fn load(&self) -> Result<(), DaemonError> {
        let mut state = self.state.lock().await;
        state.load_workflows().await?;
        Ok(())
    }
}

fn ok(text: String) -> rmcp::model::CallToolResult {
    rmcp::model::CallToolResult::success(vec![Content::text(text)])
}

fn err_result(msg: String) -> rmcp::model::CallToolResult {
    rmcp::model::CallToolResult::error(vec![Content::text(msg)])
}

#[tool_router]
impl ZerochainMcpServer {
    #[tool(
        name = "zerochain_init",
        description = "Create a new workflow with numbered stages. Optionally specify a template name (code-review, research, implement)."
    )]
    async fn init_workflow(
        &self,
        rmcp::handler::server::wrapper::Parameters(InitParams { name, template }): rmcp::handler::server::wrapper::Parameters<InitParams>,
    ) -> rmcp::model::CallToolResult {
        if !is_valid_workflow_name(&name) {
            return err_result("invalid workflow name: must be 1-128 chars, alphanumeric plus -_.".into());
        }
        let mut state = self.state.lock().await;
        match state.init_workflow(crate::state::InitWorkflowParams {
            name: &name,
            path: None,
            template: template.as_deref(),
        }).await {
            Ok(_) => ok(format!("initialized workflow: {name}")),
            Err(e) => err_result(format!("init failed: {e}")),
        }
    }

    #[tool(
        name = "zerochain_run",
        description = "Execute the next pending stage in a workflow, or a specific stage by ID."
    )]
    async fn run_stage(
        &self,
        rmcp::handler::server::wrapper::Parameters(RunParams { workflow_id, stage }): rmcp::handler::server::wrapper::Parameters<RunParams>,
    ) -> rmcp::model::CallToolResult {
        use zerochain_core::stage::StageId;
        use zerochain_fs::{acquire_lock, clean_output, mark_complete};

        let mut state = self.state.lock().await;
        let workflow = match state.get_workflow(&workflow_id).cloned() {
            Some(w) => w,
            None => return err_result(format!("workflow not found: {workflow_id}")),
        };

        let plan = workflow.execution_plan();
        if plan.is_complete() {
            return ok("workflow is already complete".into());
        }

        let stage_id = match &stage {
            Some(s) => match StageId::parse(s) {
                Ok(id) => id,
                Err(e) => return err_result(format!("invalid stage ID: {e}")),
            },
            None => match plan.next_stage().cloned() {
                Some(id) => id,
                None => return err_result("no pending stages".into()),
            },
        };

        let stage = match workflow.stage_by_id(&stage_id).cloned() {
            Some(s) => s,
            None => return err_result(format!("stage not found: {}", stage_id.raw)),
        };

        if let Err(e) = acquire_lock(&stage.path).await {
            return err_result(format!("lock failed: {e}"));
        }
        if let Err(e) = clean_output(&stage.path).await {
            return err_result(format!("clean failed: {e}"));
        }

        if let Err(e) = state.execute_stage(&workflow_id, &stage).await {
            let error_marker = stage.path.join(".error");
            if let Err(e2) = tokio::fs::write(&error_marker, format!("{e}")).await {
                tracing::warn!(path = %error_marker.display(), error = %e2, "failed to write error marker");
            }
            return err_result(format!("stage execution failed: {e}"));
        }

        if let Err(e) = mark_complete(&stage.path, None).await {
            return err_result(format!("mark complete failed: {e}"));
        }

        if let Err(e) = state.reload_workflow(&workflow_id).await {
            return err_result(format!("reload failed: {e}"));
        }

        ok(format!("stage {} complete in {}", stage_id.raw, workflow_id))
    }

    #[tool(
        name = "zerochain_status",
        description = "Show workflow status. Pass a workflow_id for details, or omit to list all."
    )]
    async fn status(
        &self,
        rmcp::handler::server::wrapper::Parameters(StatusParams { workflow_id }): rmcp::handler::server::wrapper::Parameters<StatusParams>,
    ) -> rmcp::model::CallToolResult {
        let state = self.state.lock().await;

        match workflow_id {
            Some(wid) => {
                let workflow = match state.get_workflow(&wid) {
                    Some(w) => w,
                    None => return err_result(format!("workflow not found: {wid}")),
                };
                let plan = workflow.execution_plan();
                let mut lines = vec![
                    format!("id:       {}", workflow.id),
                    format!("root:     {}", workflow.root.display()),
                    format!("stages:   {}", workflow.stages.len()),
                    format!("complete: {}", plan.is_complete()),
                    format!(
                        "next:     {}",
                        plan.next_stage()
                            .map(|s| s.raw.as_str())
                            .unwrap_or("none")
                    ),
                ];
                for stage in &workflow.stages {
                    let marker = if stage.is_complete {
                        "done"
                    } else if stage.is_error {
                        "error"
                    } else if stage.human_gate {
                        "gate"
                    } else {
                        "pending"
                    };
                    lines.push(format!("  {} [{}]", stage.id.raw, marker));
                }
                ok(lines.join("\n"))
            }
            None => {
                let workflows = state.list_workflows();
                if workflows.is_empty() {
                    return ok("no workflows".into());
                }
                let lines: Vec<String> = workflows
                    .iter()
                    .map(|(id, status)| format!("{id}\t{status}"))
                    .collect();
                ok(lines.join("\n"))
            }
        }
    }

    #[tool(
        name = "zerochain_list",
        description = "List all workflows and their status."
    )]
    async fn list_workflows(&self) -> rmcp::model::CallToolResult {
        let state = self.state.lock().await;
        let workflows = state.list_workflows();
        if workflows.is_empty() {
            return ok("no workflows".into());
        }
        let lines: Vec<String> = workflows
            .iter()
            .map(|(id, status)| format!("{id}\t{status}"))
            .collect();
        ok(lines.join("\n"))
    }

    #[tool(
        name = "zerochain_approve",
        description = "Approve a stage that is waiting at a human gate."
    )]
    async fn approve_stage(
        &self,
        rmcp::handler::server::wrapper::Parameters(StageParams { workflow_id, stage_id }): rmcp::handler::server::wrapper::Parameters<StageParams>,
    ) -> rmcp::model::CallToolResult {
        let mut state = self.state.lock().await;
        match state.mark_stage_complete(&workflow_id, &stage_id).await {
            Ok(_) => ok(format!("approved: {workflow_id} / {stage_id}")),
            Err(e) => err_result(format!("approve failed: {e}")),
        }
    }

    #[tool(
        name = "zerochain_reject",
        description = "Reject a stage and mark it as error with optional feedback."
    )]
    async fn reject_stage(
        &self,
        rmcp::handler::server::wrapper::Parameters(RejectParams { workflow_id, stage_id, feedback }): rmcp::handler::server::wrapper::Parameters<RejectParams>,
    ) -> rmcp::model::CallToolResult {
        let mut state = self.state.lock().await;
        match state.mark_stage_error(&workflow_id, &stage_id, feedback.as_deref()).await {
            Ok(_) => ok(format!("rejected: {workflow_id} / {stage_id}")),
            Err(e) => err_result(format!("reject failed: {e}")),
        }
    }

    #[tool(
        name = "zerochain_snapshot",
        description = "Create a CoW snapshot of a stage's current state for rollback."
    )]
    async fn snapshot_stage(
        &self,
        rmcp::handler::server::wrapper::Parameters(StageParams { workflow_id, stage_id }): rmcp::handler::server::wrapper::Parameters<StageParams>,
    ) -> rmcp::model::CallToolResult {
        let state = self.state.lock().await;
        match state.snapshot_stage(&workflow_id, &stage_id).await {
            Ok(path) => ok(format!("snapshot created: {}", path.display())),
            Err(e) => err_result(format!("snapshot failed: {e}")),
        }
    }

    #[tool(
        name = "zerochain_restore",
        description = "Restore a stage from its latest CoW snapshot."
    )]
    async fn restore_stage(
        &self,
        rmcp::handler::server::wrapper::Parameters(StageParams { workflow_id, stage_id }): rmcp::handler::server::wrapper::Parameters<StageParams>,
    ) -> rmcp::model::CallToolResult {
        let state = self.state.lock().await;
        match state.restore_stage(&workflow_id, &stage_id).await {
            Ok(()) => ok(format!("restored: {workflow_id} / {stage_id}")),
            Err(e) => err_result(format!("restore failed: {e}")),
        }
    }
}

impl ServerHandler for ZerochainMcpServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::InitializeResult::default()
    }
}

pub async fn run_stdio_server(workspace: PathBuf) -> anyhow::Result<()> {
    use rmcp::ServiceExt;

    let server = ZerochainMcpServer::new(workspace);
    server.load().await?;

    let transport = (tokio::io::stdin(), tokio::io::stdout());
    let service = server
        .serve(transport)
        .await
        .map_err(|e| anyhow::anyhow!("MCP server error: {e}"))?;
    service.waiting().await?;
    Ok(())
}
