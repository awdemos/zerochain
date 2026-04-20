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

use zerochain_engine::{AppState, DaemonError};

pub struct ZerochainMcpServer {
    #[allow(dead_code)] // read by #[tool_router] macro-generated code
    tool_router: ToolRouter<Self>,
    state: Arc<Mutex<AppState>>,
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
    #[must_use] pub fn new(workspace: impl AsRef<std::path::Path>) -> Self {
        let state = AppState::new(workspace.as_ref());
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

fn tool_success(text: String) -> rmcp::model::CallToolResult {
    rmcp::model::CallToolResult::success(vec![Content::text(text)])
}

fn tool_error(msg: String) -> rmcp::model::CallToolResult {
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
        rmcp::handler::server::wrapper::Parameters(zerochain_engine::InitWorkflowRequest { name, template, .. }): rmcp::handler::server::wrapper::Parameters<zerochain_engine::InitWorkflowRequest>,
    ) -> rmcp::model::CallToolResult {
        if !is_valid_workflow_name(&name) {
            return tool_error("invalid workflow name: must be 1-128 chars, alphanumeric plus -_.".into());
        }
        let mut state = self.state.lock().await;
        match state.init_workflow(zerochain_engine::InitWorkflowParams {
            name: &name,
            path: None,
            template: template.as_deref(),
        }).await {
            Ok(_) => tool_success(format!("initialized workflow: {name}")),
            Err(e) => tool_error(format!("init failed: {e}")),
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


        let mut state = self.state.lock().await;
        let Some(workflow) = state.get_workflow(&workflow_id).cloned() else {
            return tool_error(format!("workflow not found: {workflow_id}"));
        };

        let plan = workflow.execution_plan();
        if plan.is_complete() {
            return tool_success("workflow is already complete".into());
        }

        let stage_id = match &stage {
            Some(s) => match StageId::parse(s) {
                Ok(id) => id,
                Err(e) => return tool_error(format!("invalid stage ID: {e}")),
            },
            None => match plan.next_stage().cloned() {
                Some(id) => id,
                None => return tool_error("no pending stages".into()),
            },
        };

        let Some(_stage) = workflow.stage_by_id(&stage_id).cloned() else {
            return tool_error(format!("stage not found: {}", stage_id.raw));
        };

        match state.run_stage(&workflow_id, &stage_id.raw).await {
            Ok(()) => tool_success(format!("stage {} complete in {}", stage_id.raw, workflow_id)),
            Err(e) => tool_error(format!("stage execution failed: {e}")),
        }
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

        if let Some(wid) = workflow_id {
            let Some(workflow) = state.get_workflow(&wid) else {
                return tool_error(format!("workflow not found: {wid}"));
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
                        .map_or("none", |s| s.raw.as_str())
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
            tool_success(lines.join("\n"))
        } else {
            let workflows = state.list_workflows();
            if workflows.is_empty() {
                return tool_success("no workflows".into());
            }
            let lines: Vec<String> = workflows
                .iter()
                .map(|(id, status)| format!("{id}\t{status}"))
                .collect();
            tool_success(lines.join("\n"))
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
            return tool_success("no workflows".into());
        }
        let lines: Vec<String> = workflows
            .iter()
            .map(|(id, status)| format!("{id}\t{status}"))
            .collect();
        tool_success(lines.join("\n"))
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
            Ok(()) => {
                if let Err(e) = state.reload_workflow(&workflow_id).await {
                    tracing::warn!(error = %e, "failed to reload workflow after approve");
                }
                tool_success(format!("approved: {workflow_id} / {stage_id}"))
            }
            Err(e) => tool_error(format!("approve failed: {e}")),
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
            Ok(()) => {
                if let Err(e) = state.reload_workflow(&workflow_id).await {
                    tracing::warn!(error = %e, "failed to reload workflow after reject");
                }
                tool_success(format!("rejected: {workflow_id} / {stage_id}"))
            }
            Err(e) => tool_error(format!("reject failed: {e}")),
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
            Ok(path) => tool_success(format!("snapshot created: {}", path.display())),
            Err(e) => tool_error(format!("snapshot failed: {e}")),
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
            Ok(()) => tool_success(format!("restored: {workflow_id} / {stage_id}")),
            Err(e) => tool_error(format!("restore failed: {e}")),
        }
    }
}

impl ServerHandler for ZerochainMcpServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::InitializeResult::default()
    }
}

pub async fn run_stdio_server(workspace: PathBuf) -> Result<(), zerochain_engine::DaemonError> {
    use rmcp::ServiceExt;

    let server = ZerochainMcpServer::new(workspace);
    server.load().await?;

    let transport = (tokio::io::stdin(), tokio::io::stdout());
    let service = server
        .serve(transport)
        .await
        .map_err(|e| zerochain_engine::DaemonError::Llm(zerochain_llm::error::LLMError::Config(
            format!("MCP server error: {e}")
        )))?;
    service.waiting().await
        .map_err(|e| zerochain_engine::DaemonError::Llm(zerochain_llm::error::LLMError::Config(
            format!("MCP transport error: {e}")
        )))?;
    Ok(())
}
