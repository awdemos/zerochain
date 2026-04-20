use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use zerochain_core::stage::StageId;
use zerochain_broker::{Broker, BrokerMessage};

use crate::handlers::{PromptRequest, SimpleMessage};
use crate::state::ServerState;

pub async fn send(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
    Json(body): Json<PromptRequest>,
) -> impl IntoResponse {
    if let Err(e) = StageId::parse(&stage_raw) {
        return (
            StatusCode::BAD_REQUEST,
            Json(SimpleMessage {
                message: format!("invalid stage id: {e}"),
            }),
        )
            .into_response();
    }
    let Some(broker) = state.broker() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SimpleMessage {
                message: "broker not configured".into(),
            }),
        )
            .into_response();
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

pub async fn poll(
    State(state): State<ServerState>,
    Path((id, stage_raw)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = StageId::parse(&stage_raw) {
        return (
            StatusCode::BAD_REQUEST,
            Json(SimpleMessage {
                message: format!("invalid stage id: {e}"),
            }),
        )
            .into_response();
    }
    let Some(broker) = state.broker() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SimpleMessage {
                message: "broker not configured".into(),
            }),
        )
            .into_response();
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
        Ok(Some(msg)) => Json::<BrokerMessage>(msg).into_response(),
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
