use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use zerochain_core::jj;
use zerochain_core::workflow::is_valid_workflow_name;
use zerochain_engine::{InitWorkflowParams, InitWorkflowRequest};

use crate::handlers::{SimpleMessage, StageStatus, WorkflowStatus};
use crate::state::ServerState;

pub async fn list(State(state): State<ServerState>) -> impl IntoResponse {
    let inner = state.inner.lock().await;
    let list = inner.list_workflows();
    Json(list
        .into_iter()
        .map(|(id, status)| SimpleMessage {
            message: format!("{id}: {status}"),
        })
        .collect::<Vec<_>>())
}

pub async fn init(
    State(state): State<ServerState>,
    Json(body): Json<InitWorkflowRequest>,
) -> impl IntoResponse {
    tracing::info!(action = "init_workflow", name = %body.name, "mutation");
    if !is_valid_workflow_name(&body.name) {
        return (
            StatusCode::BAD_REQUEST,
            Json(SimpleMessage {
                message: "invalid workflow name: must be 1-128 chars, alphanumeric plus -_."
                    .into(),
            }),
        )
            .into_response();
    }
    let mut inner = state.inner.lock().await;
    match inner
        .init_workflow(InitWorkflowParams {
            name: &body.name,
            path: None,
            template: body.template.as_deref(),
        })
        .await
    {
        Ok(wf) => {
            jj::init_repo(&state.workspace);
            let id = wf.id.clone();
            jj::auto_commit(&state.workspace, &format!("workflow init: {id}"));
            (StatusCode::CREATED, Json(SimpleMessage { message: id })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SimpleMessage {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn get(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let inner = state.inner.lock().await;
    match inner.get_workflow(&id) {
        Some(wf) => {
            let plan = wf.execution_plan();
            let status = if plan.is_complete() {
                "complete"
            } else {
                "active"
            };
            Json(WorkflowStatus {
                id: wf.id.clone(),
                status: status.to_string(),
                stages: wf
                    .stages
                    .iter()
                    .map(|s| StageStatus {
                        id: s.id.raw.clone(),
                        complete: s.is_complete,
                        error: s.is_error,
                        human_gate: s.human_gate,
                    })
                    .collect(),
            })
            .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(SimpleMessage {
                message: format!("workflow not found: {id}"),
            }),
        )
            .into_response(),
    }
}
