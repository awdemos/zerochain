use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use zerochain_broker::Broker;
use zerochain_broker::BrokerMessage;
use zerochain_cas::Cid;
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

#[derive(Deserialize)]
pub struct PromptRequest {
    pub to_stage: String,
    pub content: String,
}

#[derive(Serialize)]
pub struct ArtifactResponse {
    pub cid: String,
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
        .route("/v1/artifacts", post(upload_artifact).get(list_artifacts))
        .route("/v1/artifacts/{cid}", get(download_artifact))
        .route(
            "/v1/workflows/{id}/stages/{stage}/prompt",
            post(send_prompt),
        )
        .route(
            "/v1/workflows/{id}/stages/{stage}/poll",
            get(poll_prompts),
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


// ------------------------------------------------------------------
// Artifact routes (CAS-backed)
// ------------------------------------------------------------------

async fn upload_artifact(
    State(state): State<ServerState>,
    body: Bytes,
) -> impl IntoResponse {
    match state.cas {
        Some(cas) => match cas.put(&body).await {
            Ok(cid) => (
                StatusCode::CREATED,
                Json(ArtifactResponse { cid: cid.to_string() }),
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SimpleMessage {
                    message: format!("failed to store artifact: {e}"),
                }),
            )
                .into_response(),
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SimpleMessage {
                message: "CAS not configured".into(),
            }),
        )
            .into_response(),
    }
}

async fn list_artifacts(State(state): State<ServerState>) -> impl IntoResponse {
    match state.cas {
        Some(cas) => match cas.list().await {
            Ok(cids) => Json(
                cids.into_iter()
                    .map(|c| c.to_string())
                    .collect::<Vec<String>>(),
            )
            .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SimpleMessage {
                    message: format!("failed to list artifacts: {e}"),
                }),
            )
                .into_response(),
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SimpleMessage {
                message: "CAS not configured".into(),
            }),
        )
            .into_response(),
    }
}

async fn download_artifact(
    State(state): State<ServerState>,
    Path(cid_str): Path<String>,
) -> impl IntoResponse {
    let cid = match cid_str.parse::<Cid>() {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SimpleMessage {
                    message: format!("invalid CID: {e}"),
                }),
            )
                .into_response()
        }
    };

    match state.cas {
        Some(cas) => match cas.get(&cid).await {
            Ok(data) => data.into_response(),
            Err(zerochain_cas::CasError::NotFound(_)) => (
                StatusCode::NOT_FOUND,
                Json(SimpleMessage {
                    message: "artifact not found".into(),
                }),
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SimpleMessage {
                    message: format!("failed to retrieve artifact: {e}"),
                }),
            )
                .into_response(),
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SimpleMessage {
                message: "CAS not configured".into(),
            }),
        )
            .into_response(),
    }
}

// ------------------------------------------------------------------
// Cross-agent prompt routes (broker-backed)
// ------------------------------------------------------------------

async fn send_prompt(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
    Json(body): Json<PromptRequest>,
) -> impl IntoResponse {
    let broker = match state.broker {
        Some(b) => b,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(SimpleMessage {
                    message: "broker not configured".into(),
                }),
            )
                .into_response();
        }
    };

    let cas = match state.cas {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(SimpleMessage {
                    message: "CAS not configured".into(),
                }),
            )
                .into_response();
        }
    };

    // Store the prompt content in CAS
    let prompt_cid = match cas.put(body.content.as_bytes()).await {
        Ok(cid) => cid,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SimpleMessage {
                    message: format!("failed to store prompt: {e}"),
                }),
            )
                .into_response();
        }
    };

    // Publish broker message
    let subject = format!("zerochain.{id}.{}", body.to_stage);
    let msg = BrokerMessage::new(&id, &stage_raw, &body.to_stage, prompt_cid.clone());

    match broker.publish(&subject, msg).await {
        Ok(()) => Json(SimpleMessage {
            message: format!("prompt sent to {} (cid: {})", body.to_stage, prompt_cid),
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SimpleMessage {
                message: format!("failed to publish prompt: {e}"),
            }),
        )
            .into_response(),
    }
}

async fn poll_prompts(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
) -> impl IntoResponse {
    let broker = match state.broker {
        Some(b) => b,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(SimpleMessage {
                    message: "broker not configured".into(),
                }),
            )
                .into_response();
        }
    };

    let subject = format!("zerochain.{id}.{stage_raw}");
    let mut rx = match broker.subscribe(&subject).await {
        Ok(rx) => rx,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SimpleMessage {
                    message: format!("failed to subscribe: {e}"),
                }),
            )
                .into_response();
        }
    };

    // For MVP, return the first available message or empty.
    // In production this would be an SSE stream.
    match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
        Ok(Some(msg)) => Json(msg).into_response(),
        Ok(None) => Json(SimpleMessage {
            message: "no messages".into(),
        })
        .into_response(),
        Err(_) => Json(SimpleMessage {
            message: "timeout".into(),
        })
        .into_response(),
    }
}
