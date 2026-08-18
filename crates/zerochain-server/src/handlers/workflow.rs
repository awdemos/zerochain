use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use std::path::PathBuf;
use zerochain_core::jj;
use zerochain_core::workflow::is_valid_workflow_name;
use zerochain_engine::InitWorkflowRequest;

use crate::handlers::{SimpleMessage, StageStatus, WorkflowStatus};
use crate::state::ServerState;

#[derive(Deserialize)]
pub struct ExportOkfQuery {
    pub output: Option<PathBuf>,
}

pub async fn list(State(state): State<ServerState>) -> impl IntoResponse {
    let registry = state.registry.read().await;
    let list = registry.list_workflows().await;
    Json(
        list.into_iter()
            .map(|(id, status)| SimpleMessage {
                message: format!("{id}: {status}"),
            })
            .collect::<Vec<_>>(),
    )
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
                message: "invalid workflow name: must be 1-128 chars, alphanumeric plus -_.".into(),
            }),
        )
            .into_response();
    }
    let registry = state.registry.read().await;
    match registry.init_workflow(body.name, body.template).await {
        Ok(wf) => {
            jj::init_repo(&state.workspace).await;
            let id = wf.id.clone();
            jj::auto_commit(&state.workspace, &format!("workflow init: {id}")).await;
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

pub async fn get(State(state): State<ServerState>, Path(id): Path<String>) -> impl IntoResponse {
    let handle = {
        let registry = state.registry.read().await;
        match registry.get_or_create(&id).await {
            Ok(h) => h,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(SimpleMessage {
                        message: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
    };

    match handle.get_workflow(id.clone()).await {
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

pub async fn export_okf(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    Query(query): Query<ExportOkfQuery>,
) -> impl IntoResponse {
    let output_dir = query.output.unwrap_or_else(|| {
        state.workspace.join(format!("{}-okf", id))
    });
    let registry = state.registry.read().await;
    match registry.export_okf(id.clone(), output_dir.clone()).await {
        Ok(_) => Json(SimpleMessage {
            message: format!("exported OKF bundle to {}", output_dir.display()),
        })
            .into_response(),
        Err(e) => match e {
            zerochain_engine::DaemonError::WorkflowNotFound(_) => (
                StatusCode::NOT_FOUND,
                Json(SimpleMessage {
                    message: e.to_string(),
                }),
            )
                .into_response(),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SimpleMessage {
                    message: e.to_string(),
                }),
            )
                .into_response(),
        },
    }
}
