use axum::extract::State;
use axum::http::StatusCode;
use axum::{extract::Request, middleware::Next, response::Response};

use crate::state::ServerState;

/// Bearer token middleware. Skips auth when:
/// - The request path is `/v1/health`
/// - No API key is configured on the server state
pub async fn require_api_key(
    State(state): State<ServerState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if request.uri().path() == "/v1/health" {
        return Ok(next.run(request).await);
    }

    let expected = match &state.api_key {
        Some(key) => key,
        None => return Ok(next.run(request).await),
    };

    let header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match header {
        Some(value) if value.starts_with("Bearer ") => {
            let token = &value[7..];
            if token == expected {
                Ok(next.run(request).await)
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}
