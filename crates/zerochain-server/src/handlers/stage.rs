use axum::extract::{OriginalUri, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use zerochain_core::jj;
use zerochain_core::stage::StageId;

use crate::handlers::{RejectRequest, SimpleMessage};
use crate::state::ServerState;

pub async fn run_next(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let handle = {
        let mut registry = state.registry.write().await;
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

    let wf = match handle.get_workflow(id.clone()).await {
        Some(wf) => wf,
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

    run_stage_by_id(&state, &id, &next_stage.raw).await
}

pub async fn run(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
) -> impl IntoResponse {
    run_stage_by_id(&state, &id, &stage_raw).await
}

async fn run_stage_by_id(
    state: &ServerState,
    id: &str,
    stage_raw: &str,
) -> axum::response::Response {
    tracing::info!(action = "run_stage", workflow = %id, stage = %stage_raw, "mutation");
    let sid = match StageId::parse(stage_raw) {
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

    let handle = {
        let mut registry = state.registry.write().await;
        match registry.get_or_create(id).await {
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

    let wf = match handle.get_workflow(id.to_string()).await {
        Some(wf) => wf,
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

    let stage = match wf.stage_by_id(&sid) {
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

    let stage_raw = stage.id.raw.clone();
    let result = handle.run_stage(id.to_string(), stage_raw.clone()).await;

    match result {
        Ok(()) => {
            jj::commit_stage_complete(&state.workspace, id, &stage_raw);
            Json(SimpleMessage {
                message: format!("stage {stage_raw} complete"),
            })
            .into_response()
        }
        Err(e) => {
            jj::commit_stage_error(&state.workspace, id, &stage_raw);
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

pub async fn approve(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
) -> impl IntoResponse {
    tracing::info!(action = "approve", workflow = %id, stage = %stage_raw, "mutation");
    if let Err(e) = StageId::parse(&stage_raw) {
        return (
            StatusCode::BAD_REQUEST,
            Json(SimpleMessage {
                message: format!("invalid stage id: {e}"),
            }),
        )
            .into_response();
    }

    let handle = {
        let mut registry = state.registry.write().await;
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

    match handle.mark_stage_complete(id.clone(), stage_raw.clone()).await {
        Ok(()) => {
            if let Err(e) = handle.reload_workflow(id.clone()).await {
                tracing::warn!(error = %e, "failed to reload workflow after approve");
            }
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

pub async fn reject(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
    Json(body): Json<RejectRequest>,
) -> impl IntoResponse {
    tracing::info!(action = "reject", workflow = %id, stage = %stage_raw, "mutation");
    if let Err(e) = StageId::parse(&stage_raw) {
        return (
            StatusCode::BAD_REQUEST,
            Json(SimpleMessage {
                message: format!("invalid stage id: {e}"),
            }),
        )
            .into_response();
    }

    let handle = {
        let mut registry = state.registry.write().await;
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

    match handle
        .mark_stage_error(id.clone(), stage_raw.clone(), body.feedback)
        .await
    {
        Ok(()) => {
            if let Err(e) = handle.reload_workflow(id.clone()).await {
                tracing::warn!(error = %e, "failed to reload workflow after reject");
            }
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

async fn read_stage_file(
    state: &ServerState,
    id: &str,
    stage_raw: &str,
    filename: &str,
    not_found_msg: String,
) -> axum::response::Response {
    let handle = {
        let mut registry = state.registry.write().await;
        match registry.get_or_create(id).await {
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

    let sid = match StageId::parse(stage_raw) {
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

    let wf = match handle.get_workflow(id.to_string()).await {
        Some(wf) => wf,
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

    match wf.stage_by_id(&sid) {
        Some(stage) => {
            let path = stage.output_path.join(filename);
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => content.into_response(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                    StatusCode::NOT_FOUND,
                    Json(SimpleMessage {
                        message: not_found_msg,
                    }),
                )
                    .into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(SimpleMessage {
                        message: format!("failed to read stage file: {e}"),
                    }),
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

pub async fn read_file_route(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
    OriginalUri(uri): OriginalUri,
) -> impl IntoResponse {
    let (filename, not_found_msg) = if uri.path().contains("/reasoning/") {
        ("reasoning.md", "reasoning not available")
    } else {
        ("result.md", "output not available")
    };
    read_stage_file(&state, &id, &stage_raw, filename, not_found_msg.into()).await
}
