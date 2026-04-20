use axum::extract::State;
use axum::http::StatusCode;
use axum::{extract::Request, middleware::Next, response::Response};

use crate::state::ServerState;

/// Constant-time string comparison to mitigate timing attacks.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        result |= x ^ y;
    }
    result == 0
}

/// Bearer token middleware. Skips auth when:
/// - The request path is `/v1/health`
/// - No API key is configured on the server state
///
/// # Errors
///
/// Returns `StatusCode::UNAUTHORIZED` when the bearer token is missing or invalid.
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
            if constant_time_eq(token, expected) {
                tracing::info!(path = %request.uri().path(), "auth success");
                Ok(next.run(request).await)
            } else {
                tracing::warn!(path = %request.uri().path(), "auth failed: invalid token");
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        _ => {
            tracing::warn!(path = %request.uri().path(), "auth failed: missing credentials");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}
