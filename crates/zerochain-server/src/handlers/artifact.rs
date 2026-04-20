use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use zerochain_cas::Cid;

use crate::handlers::{ArtifactResponse, SimpleMessage};
use crate::state::ServerState;

pub async fn upload(
    State(state): State<ServerState>,
    body: Bytes,
) -> impl IntoResponse {
    let Some(cas) = state.cas() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SimpleMessage {
                message: "CAS not configured".into(),
            }),
        )
            .into_response();
    };
    match cas.put(&body).await {
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
    }
}

pub async fn list(State(state): State<ServerState>) -> impl IntoResponse {
    let Some(cas) = state.cas() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SimpleMessage {
                message: "CAS not configured".into(),
            }),
        )
            .into_response();
    };
    match cas.list().await {
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
    }
}

pub async fn download(
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

    let Some(cas) = state.cas() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SimpleMessage {
                message: "CAS not configured".into(),
            }),
        )
            .into_response();
    };
    match cas.get(&cid).await {
        Ok(data) => data.into_response(),
        Err(e) if e.is_not_found() => (
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
    }
}
