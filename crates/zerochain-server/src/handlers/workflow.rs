use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use std::path::{Path as FsPath, PathBuf};
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
    jj::init_repo(&state.workspace).await;
    let registry = state.registry.read().await;
    match registry
        .init_workflow(body.name, body.template, body.parents)
        .await
    {
        Ok(wf) => {
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
            Err(zerochain_engine::DaemonError::WorkflowNotFound(_)) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(SimpleMessage {
                        message: format!("workflow not found: {id}"),
                    }),
                )
                    .into_response();
            }
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
    // The caller-supplied output path becomes a create_dir_all + write target,
    // so it must be proven to live strictly inside the workspace before it
    // reaches the engine. Escapes (absolute paths, `..`, symlinks) are
    // rejected outright rather than falling back to a default.
    let output_dir = match &query.output {
        Some(requested) => match resolve_in_workspace(&state.workspace, requested) {
            Some(resolved) => resolved,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(SimpleMessage {
                        message: format!(
                            "invalid output path: {} must be inside the workspace",
                            requested.display()
                        ),
                    }),
                )
                    .into_response();
            }
        },
        None => state.workspace.join(format!("{}-okf", id)),
    };
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

/// Resolve `requested` to an absolute path guaranteed to live strictly inside
/// `workspace`, even when `requested` does not exist yet.
///
/// Returns `None` when the resolved path would escape the workspace (via
/// `..`, an absolute path outside it, or a symlink) or is the workspace root
/// itself. Relative paths are anchored at the workspace. Symlink safety comes
/// from canonicalizing the nearest existing ancestor; not-yet-existing tail
/// components cannot hide symlinks because they do not exist yet.
fn resolve_in_workspace(workspace: &FsPath, requested: &FsPath) -> Option<PathBuf> {
    let root = workspace.canonicalize().ok()?;
    let requested = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };

    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut existing = requested.as_path();
    loop {
        match existing.canonicalize() {
            Ok(resolved_existing) => {
                let mut resolved = resolved_existing;
                for component in tail.into_iter().rev() {
                    resolved.push(component);
                }
                return (resolved.starts_with(&root) && resolved != root).then_some(resolved);
            }
            Err(_) => {
                tail.push(existing.file_name()?.to_os_string());
                existing = existing.parent()?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_in_workspace;
    use std::fs;
    use std::path::Path;

    #[test]
    fn resolves_new_directory_inside_workspace() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        // Compare against the canonicalized workspace: hosts may keep the
        // temp dir below a symlinked path (e.g. /home -> /var/home).
        let root = tmp.path().canonicalize().expect("canonicalize root");
        let resolved = resolve_in_workspace(tmp.path(), Path::new("bundle/sub")).expect("resolve");
        assert_eq!(resolved, root.join("bundle/sub"));
    }

    #[test]
    fn resolves_relative_dot_path_to_workspace_child() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonicalize root");
        let resolved = resolve_in_workspace(tmp.path(), Path::new("./out")).expect("resolve");
        assert_eq!(resolved, root.join("out"));
    }

    #[test]
    fn rejects_parent_directory_escape() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        assert!(resolve_in_workspace(tmp.path(), Path::new("../evil")).is_none());
        assert!(resolve_in_workspace(tmp.path(), Path::new("sub/../../evil")).is_none());
    }

    #[test]
    fn rejects_absolute_path_outside_workspace() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let outside = tmp.path().parent().expect("parent").join("evil-export");
        assert!(resolve_in_workspace(tmp.path(), &outside).is_none());
    }

    #[test]
    fn rejects_workspace_root_itself() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        assert!(resolve_in_workspace(tmp.path(), Path::new(".")).is_none());
        assert!(resolve_in_workspace(tmp.path(), tmp.path()).is_none());
    }

    #[test]
    fn rejects_symlink_escape() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let outside = tempfile::TempDir::new().expect("outside tempdir");
        let link = tmp.path().join("link-out");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");
            let target = outside.path().join("bundle");
            assert!(resolve_in_workspace(tmp.path(), &link.join("bundle")).is_none());
            assert!(resolve_in_workspace(tmp.path(), &target).is_none());
        }
    }

    #[test]
    fn allows_symlink_that_stays_inside_workspace() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let real = tmp.path().join("real");
        fs::create_dir(&real).expect("mkdir");
        let link = tmp.path().join("link-in");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, &link).expect("symlink");
            let resolved = resolve_in_workspace(tmp.path(), &link.join("bundle")).expect("resolve");
            assert_eq!(
                resolved,
                real.canonicalize()
                    .expect("canonicalize real")
                    .join("bundle")
            );
        }
    }
}
