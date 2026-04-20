use axum::Router;
use axum::routing::{get, post};
use axum::middleware;
use serde::{Deserialize, Serialize};

use crate::auth;
use crate::state::ServerState;

pub mod artifact;
pub mod health;
pub mod prompt;
pub mod stage;
pub mod workflow;



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
    let protected = Router::new()
        .route("/v1/workflows", get(workflow::list).post(workflow::init))
        .route("/v1/workflows/{id}", get(workflow::get))
        .route("/v1/workflows/{id}/run", post(stage::run_next))
        .route("/v1/workflows/{id}/run/{stage}", post(stage::run))
        .route("/v1/workflows/{id}/approve/{stage}", post(stage::approve))
        .route("/v1/workflows/{id}/reject/{stage}", post(stage::reject))
        .route(
            "/v1/workflows/{id}/output/{stage}",
            get(stage::read_file_route),
        )
        .route(
            "/v1/workflows/{id}/reasoning/{stage}",
            get(stage::read_file_route),
        )
        .route("/v1/artifacts", post(artifact::upload).get(artifact::list))
        .route("/v1/artifacts/{cid}", get(artifact::download))
        .route(
            "/v1/workflows/{id}/stages/{stage}/prompt",
            post(prompt::send),
        )
        .route(
            "/v1/workflows/{id}/stages/{stage}/poll",
            get(prompt::poll),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_api_key,
        ));

    Router::new()
        .route("/v1/health", get(health::health))
        .merge(protected)
        .with_state(state)
}
