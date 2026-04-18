use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use zerochain_core::stage::StageId;

use crate::jj;
use crate::state::ServerState;

#[derive(Deserialize)]
pub struct InitWorkflowRequest {
    pub name: String,
    pub template: Option<String>,
}

#[derive(Deserialize)]
pub struct RejectRequest {
    pub feedback: Option<String>,
}

#[derive(Serialize)]
pub struct WorkflowStatus {
    pub id: String,
    pub status: String,
    pub stages: Vec<StageStatus>,
}

#[derive(Serialize)]
pub struct StageStatus {
    pub id: String,
    pub complete: bool,
    pub error: bool,
    pub human_gate: bool,
}

#[derive(Serialize)]
pub struct SimpleMessage {
    pub message: String,
}

pub fn routes(state: ServerState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/workflows", get(list_workflows).post(init_workflow))
        .route("/v1/workflows/{id}", get(get_workflow))
        .route("/v1/workflows/{id}/run", post(run_next))
        .route("/v1/workflows/{id}/run/{stage}", post(run_stage))
        .route("/v1/workflows/{id}/approve/{stage}", post(approve))
        .route("/v1/workflows/{id}/reject/{stage}", post(reject))
        .route(
            "/v1/workflows/{id}/output/{stage}",
            get(read_output),
        )
        .route(
            "/v1/workflows/{id}/reasoning/{stage}",
            get(read_reasoning),
        )
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn list_workflows(State(state): State<ServerState>) -> impl IntoResponse {
    let inner = state.inner.lock().await;
    let list = inner.list_workflows();
    Json(list
        .into_iter()
        .map(|(id, status)| SimpleMessage {
            message: format!("{id}: {status}"),
        })
        .collect::<Vec<_>>())
}

async fn init_workflow(
    State(state): State<ServerState>,
    Json(body): Json<InitWorkflowRequest>,
) -> impl IntoResponse {
    let mut inner = state.inner.lock().await;
    match inner
        .init_workflow(None, &body.name, body.template.as_deref())
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

async fn get_workflow(
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

async fn run_next(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut inner = state.inner.lock().await;
    let wf = match inner.get_workflow(&id) {
        Some(wf) => wf.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(SimpleMessage {
                    message: format!("workflow not found: {id}"),
                }),
            )
                .into_response()
        }
    };

    let plan = wf.execution_plan();
    let next_stage = match plan.next_stage() {
        Some(stage) => stage.clone(),
        None => {
            return Json(SimpleMessage {
                message: "no pending stages".into(),
            })
            .into_response()
        }
    };

    let stage = match wf.stage_by_name(&next_stage.name) {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(SimpleMessage {
                    message: format!("stage not found: {next_stage}"),
                }),
            )
                .into_response()
        }
    };

    let stage_raw = stage.id.raw.clone();
    match inner.execute_stage(&id, &stage).await {
        Ok(()) => {
            if let Err(e) = inner.mark_stage_complete(&id, &stage_raw).await {
                tracing::warn!(error = %e, "failed to mark stage complete");
            }
            let _ = inner.reload_workflow(&id).await;
            drop(inner);
            jj::commit_stage_complete(&state.workspace, &id, &stage_raw);
            Json(SimpleMessage {
                message: format!("stage {stage_raw} complete"),
            })
            .into_response()
        }
        Err(e) => {
            let _ = inner.mark_stage_error(&id, &stage_raw, None).await;
            let _ = inner.reload_workflow(&id).await;
            drop(inner);
            jj::commit_stage_error(&state.workspace, &id, &stage_raw);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SimpleMessage {
                    message: format!("stage {stage_raw} failed: {e}"),
                }),
            )
                .into_response()
        }
    }
}

async fn run_stage(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
) -> impl IntoResponse {
    let sid = match StageId::parse(&stage_raw) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SimpleMessage {
                    message: format!("invalid stage id: {e}"),
                }),
            )
                .into_response()
        }
    };

    let mut inner = state.inner.lock().await;
    let stage = match inner.get_workflow(&id).and_then(|wf| wf.stage_by_id(&sid)) {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(SimpleMessage {
                    message: format!("stage not found: {stage_raw}"),
                }),
            )
                .into_response()
        }
    };

    match inner.execute_stage(&id, &stage).await {
        Ok(()) => {
            if let Err(e) = inner.mark_stage_complete(&id, &stage_raw).await {
                tracing::warn!(error = %e, "failed to mark stage complete");
            }
            let _ = inner.reload_workflow(&id).await;
            drop(inner);
            jj::commit_stage_complete(&state.workspace, &id, &stage_raw);
            Json(SimpleMessage {
                message: format!("stage {stage_raw} complete"),
            })
            .into_response()
        }
        Err(e) => {
            let _ = inner.mark_stage_error(&id, &stage_raw, None).await;
            let _ = inner.reload_workflow(&id).await;
            drop(inner);
            jj::commit_stage_error(&state.workspace, &id, &stage_raw);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SimpleMessage {
                    message: format!("stage {stage_raw} failed: {e}"),
                }),
            )
                .into_response()
        }
    }
}

async fn approve(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
) -> impl IntoResponse {
    let mut inner = state.inner.lock().await;
    match inner.mark_stage_complete(&id, &stage_raw).await {
        Ok(()) => {
            if let Err(e) = inner.reload_workflow(&id).await {
                tracing::warn!(error = %e, "failed to reload workflow after approve");
            }
            drop(inner);
            jj::commit_stage_complete(&state.workspace, &id, &stage_raw);
            Json(SimpleMessage {
                message: format!("stage {stage_raw} approved"),
            })
            .into_response()
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

async fn reject(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
    Json(body): Json<RejectRequest>,
) -> impl IntoResponse {
    let mut inner = state.inner.lock().await;
    match inner
        .mark_stage_error(&id, &stage_raw, body.feedback.as_deref())
        .await
    {
        Ok(()) => {
            if let Err(e) = inner.reload_workflow(&id).await {
                tracing::warn!(error = %e, "failed to reload workflow after reject");
            }
            drop(inner);
            jj::commit_stage_error(&state.workspace, &id, &stage_raw);
            Json(SimpleMessage {
                message: format!("stage {stage_raw} rejected"),
            })
            .into_response()
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

async fn read_output(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
) -> impl IntoResponse {
    let inner = state.inner.lock().await;
    let sid = match StageId::parse(&stage_raw) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SimpleMessage {
                    message: format!("invalid stage id: {e}"),
                }),
            )
                .into_response()
        }
    };

    match inner
        .get_workflow(&id)
        .and_then(|wf| wf.stage_by_id(&sid))
    {
        Some(stage) => {
            let result_path = stage.output_path.join("result.md");
            drop(inner);
            match tokio::fs::read_to_string(&result_path).await {
                Ok(content) => content.into_response(),
                Err(_) => (
                    StatusCode::NOT_FOUND,
                    "output not available",
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(SimpleMessage {
                message: format!("stage not found: {stage_raw}"),
            }),
        )
            .into_response(),
    }
}

async fn read_reasoning(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
) -> impl IntoResponse {
    let inner = state.inner.lock().await;
    let sid = match StageId::parse(&stage_raw) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SimpleMessage {
                    message: format!("invalid stage id: {e}"),
                }),
            )
                .into_response()
        }
    };

    match inner
        .get_workflow(&id)
        .and_then(|wf| wf.stage_by_id(&sid))
    {
        Some(stage) => {
            let path = stage.output_path.join("reasoning.md");
            drop(inner);
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => content.into_response(),
                Err(_) => (
                    StatusCode::NOT_FOUND,
                    "reasoning not available",
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(SimpleMessage {
                message: format!("stage not found: {stage_raw}"),
            }),
        )
            .into_response(),
    }
}
