use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use zerochain_fs::BtrfsCow;

use crate::handlers::{SimpleMessage, SubvolumeList};
use crate::state::ServerState;

pub async fn list(State(state): State<ServerState>, Path(id): Path<String>) -> impl IntoResponse {
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

    let wf = match handle.get_workflow(id.clone()).await {
        Some(wf) => wf,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(SimpleMessage {
                    message: format!("workflow not found: {id}"),
                }),
            )
                .into_response();
        }
    };

    if !BtrfsCow::is_btrfs_filesystem(&wf.root).await {
        return (
            StatusCode::BAD_REQUEST,
            Json(SimpleMessage {
                message: format!(
                    "workflow {} is not on a Btrfs filesystem",
                    wf.root.display()
                ),
            }),
        )
            .into_response();
    }

    let cow = BtrfsCow::new(zerochain_fs::SubvolumeMode::from_env());
    match cow.list_subvolumes(&wf.root).await {
        Ok(subvolumes) => Json(SubvolumeList {
            workflow_id: id,
            filesystem: "btrfs".to_string(),
            subvolumes,
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SimpleMessage {
                message: format!("failed to list subvolumes: {e}"),
            }),
        )
            .into_response(),
    }
}
