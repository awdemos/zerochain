use std::path::Path;

use axum::body::Body;
use http::Request;
use http::StatusCode;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use zerochain_server::routes;
use zerochain_server::state::ServerState;

struct ScopedEnv {
    key: String,
    old: Option<String>,
}

impl ScopedEnv {
    fn set(key: &str, value: &str) -> Self {
        let old = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self {
            key: key.to_string(),
            old,
        }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        match &self.old {
            Some(v) => std::env::set_var(&self.key, v),
            None => std::env::remove_var(&self.key),
        }
    }
}

fn make_app(workspace: &Path) -> axum::Router {
    let state = ServerState::new(workspace);
    routes::routes(state)
}

fn app_from_state(state: &ServerState) -> axum::Router {
    routes::routes(state.clone())
}

fn app_with_key(workspace: &Path, key: &str) -> axum::Router {
    let state = ServerState::new(workspace).with_api_key(key);
    routes::routes(state)
}

async fn body_string(body: Body) -> String {
    let bytes = body
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf8 body")
}

fn make_request(method: &str, uri: &str, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map_or(Body::empty(), |b| Body::from(b.to_string())))
        .expect("build request")
}

macro_rules! send {
    ($app:expr, $req:expr) => {{
        $app.oneshot($req).await.expect("oneshot")
    }};
}

#[tokio::test]
async fn health_returns_ok() {
    let tmp = TempDir::new().expect("tempdir");
    let app = make_app(tmp.path());

    let req = make_request("GET", "/v1/health", None);
    let resp = send!(app, req);
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_string(resp.into_body()).await;
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn list_workflows_empty() {
    let tmp = TempDir::new().expect("tempdir");
    let app = make_app(tmp.path());

    let req = make_request("GET", "/v1/workflows", None);
    let resp = send!(app, req);
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_string(resp.into_body()).await;
    assert_eq!(body, "[]");
}

#[tokio::test]
async fn init_workflow_creates_and_lists() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "test-wf"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let body: serde_json::Value =
        serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
    assert_eq!(body["message"], "test-wf");

    let req = make_request("GET", "/v1/workflows", None);
    let resp = send!(app_from_state(&state), req);
    let list: Vec<serde_json::Value> =
        serde_json::from_str(&body_string(resp.into_body()).await).expect("json list");
    assert_eq!(list.len(), 1);
    assert!(list[0]["message"].as_str().unwrap().contains("test-wf"));
}

#[tokio::test]
async fn get_workflow_returns_stages() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "staged-wf", "template": "00_spec,01_build,02_test"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request("GET", "/v1/workflows/staged-wf", None);
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value =
        serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
    assert_eq!(body["id"], "staged-wf");
    assert_eq!(body["stages"].as_array().unwrap().len(), 3);
    assert_eq!(body["stages"][0]["id"], "00_spec");
}

#[tokio::test]
async fn get_nonexistent_workflow_404s() {
    let tmp = TempDir::new().expect("tempdir");
    let app = make_app(tmp.path());

    let req = make_request("GET", "/v1/workflows/nope", None);
    let resp = send!(app, req);
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn approve_marks_stage_complete() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "approve-test", "template": "00_spec,01_build"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request("POST", "/v1/workflows/approve-test/approve/00_spec", None);
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value =
        serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
    assert!(body["message"].as_str().unwrap().contains("approved"));

    let req = make_request("GET", "/v1/workflows/approve-test", None);
    let resp = send!(app_from_state(&state), req);
    let body: serde_json::Value =
        serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
    let spec = &body["stages"].as_array().unwrap()[0];
    assert_eq!(spec["complete"], true);
}

#[tokio::test]
async fn reject_marks_stage_error() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "reject-test", "template": "00_spec"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request(
        "POST",
        "/v1/workflows/reject-test/reject/00_spec",
        Some(r#"{"feedback": "bad output"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::OK);

    let req = make_request("GET", "/v1/workflows/reject-test", None);
    let resp = send!(app_from_state(&state), req);
    let body: serde_json::Value =
        serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
    let spec = &body["stages"].as_array().unwrap()[0];
    assert_eq!(spec["error"], true);
}

#[tokio::test]
async fn read_output_missing_returns_404() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "output-test"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request("GET", "/v1/workflows/output-test/output/00_spec", None);
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn read_output_returns_content() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "read-out", "template": "00_spec"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let inner = state.inner.lock().await;
    let wf = inner.get_workflow("read-out").expect("workflow");
    let stage = wf.stage_by_name("spec").expect("stage");
    let result_path = stage.output_path.join("result.md");
    tokio::fs::write(&result_path, "hello world")
        .await
        .expect("write result");
    drop(inner);

    let req = make_request("GET", "/v1/workflows/read-out/output/00_spec", None);
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_string(resp.into_body()).await;
    assert_eq!(body, "hello world");
}

#[tokio::test]
async fn init_workflow_with_custom_template() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "custom-tpl", "template": "01_alpha,02_beta"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request("GET", "/v1/workflows/custom-tpl", None);
    let resp = send!(app_from_state(&state), req);
    let body: serde_json::Value =
        serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
    let stages = body["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 2);
    assert_eq!(stages[0]["id"], "01_alpha");
    assert_eq!(stages[1]["id"], "02_beta");
}

#[tokio::test]
async fn read_reasoning_missing_returns_404() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "reason-test", "template": "00_spec"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request("GET", "/v1/workflows/reason-test/reasoning/00_spec", None);
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn read_reasoning_returns_content() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "reason-read", "template": "00_spec"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let inner = state.inner.lock().await;
    let wf = inner.get_workflow("reason-read").expect("workflow");
    let stage = wf.stage_by_name("spec").expect("stage");
    let reasoning_path = stage.output_path.join("reasoning.md");
    tokio::fs::create_dir_all(&stage.output_path)
        .await
        .expect("mkdir output");
    tokio::fs::write(&reasoning_path, "chain of thought here")
        .await
        .expect("write reasoning");
    drop(inner);

    let req = make_request("GET", "/v1/workflows/reason-read/reasoning/00_spec", None);
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_string(resp.into_body()).await;
    assert_eq!(body, "chain of thought here");
}

#[tokio::test]
async fn reject_without_feedback() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "reject-nofb", "template": "00_spec"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request(
        "POST",
        "/v1/workflows/reject-nofb/reject/00_spec",
        Some(r"{}"),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value =
        serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
    assert!(body["message"].as_str().unwrap().contains("rejected"));

    let req = make_request("GET", "/v1/workflows/reject-nofb", None);
    let resp = send!(app_from_state(&state), req);
    let body: serde_json::Value =
        serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
    assert_eq!(body["stages"].as_array().unwrap()[0]["error"], true);
}

#[tokio::test]
async fn approve_nonexistent_stage_returns_error() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "approve-nope", "template": "00_spec"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request(
        "POST",
        "/v1/workflows/approve-nope/approve/99_nonexistent",
        None,
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn reject_nonexistent_stage_returns_error() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "reject-nope", "template": "00_spec"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request(
        "POST",
        "/v1/workflows/reject-nope/reject/99_nonexistent",
        Some(r#"{"feedback": "nope"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn read_output_for_nonexistent_stage_404s() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "output-no-stage", "template": "00_spec"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request(
        "GET",
        "/v1/workflows/output-no-stage/output/99_nonexistent",
        None,
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn run_next_returns_no_pending_when_complete() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "run-complete", "template": "00_spec"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request(
        "POST",
        "/v1/workflows/run-complete/approve/00_spec",
        None,
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::OK);

    let mut inner = state.inner.lock().await;
    inner.reload_workflow("run-complete").await.expect("reload");
    drop(inner);

    let req = make_request("POST", "/v1/workflows/run-complete/run", None);
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value =
        serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
    assert!(body["message"].as_str().unwrap().contains("no pending stages"));
}

#[tokio::test]
async fn run_next_nonexistent_workflow_404s() {
    let tmp = TempDir::new().expect("tempdir");
    let app = make_app(tmp.path());

    let req = make_request("POST", "/v1/workflows/nonexistent/run", None);
    let resp = send!(app, req);
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn run_specific_stage_nonexistent_workflow_404s() {
    let tmp = TempDir::new().expect("tempdir");
    let app = make_app(tmp.path());

    let req = make_request("POST", "/v1/workflows/nonexistent/run/00_spec", None);
    let resp = send!(app, req);
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn run_specific_stage_invalid_id_returns_400() {
    let tmp = TempDir::new().expect("tempdir");
    let state = ServerState::new(tmp.path());

    let req = make_request(
        "POST",
        "/v1/workflows",
        Some(r#"{"name": "bad-stage-id"}"#),
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = make_request(
        "POST",
        "/v1/workflows/bad-stage-id/run/not_a_valid_stage",
        None,
    );
    let resp = send!(app_from_state(&state), req);
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

mod e2e {
    use super::*;
    use axum::extract::State as AxumState;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    fn openai_response() -> Value {
        json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "model": "mock-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "The answer is 42."
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        })
    }

    async fn mock_completions(
        AxumState(count): AxumState<Arc<Mutex<usize>>>,
        _body: Json<Value>,
    ) -> Json<Value> {
        let mut n = count.lock().unwrap();
        *n += 1;
        drop(n);
        Json(openai_response())
    }

    async fn start_mock_openai() -> (String, Arc<Mutex<usize>>) {
        let call_count = Arc::new(Mutex::new(0usize));
        let count_clone = call_count.clone();

        let app = Router::new()
            .route("/v1/chat/completions", post(mock_completions))
            .with_state(count_clone);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://127.0.0.1:{port}/v1"), call_count)
    }

    #[tokio::test]
    async fn run_endpoints_with_mock_llm() {
        let tmp = TempDir::new().expect("tempdir");
        let (base_url, call_count) = start_mock_openai().await;

        let _api_key = ScopedEnv::set("OPENAI_API_KEY", "sk-test-mock-key");
        let _base_url = ScopedEnv::set("ZEROCHAIN_BASE_URL", &base_url);
        let _model = ScopedEnv::set("ZEROCHAIN_MODEL", "mock-model");

        let state = ServerState::new(tmp.path());

        let req = make_request(
            "POST",
            "/v1/workflows",
            Some(r#"{"name": "e2e-run", "template": "00_spec,01_build"}"#),
        );
        let resp = send!(app_from_state(&state), req);
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Test 1: run next pending stage
        let req = make_request("POST", "/v1/workflows/e2e-run/run", None);
        let resp = send!(app_from_state(&state), req);
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value =
            serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
        assert!(body["message"].as_str().unwrap().contains("complete"));

        assert_eq!(*call_count.lock().unwrap(), 1);

        // Verify output written
        let req = make_request("GET", "/v1/workflows/e2e-run/output/00_spec", None);
        let resp = send!(app_from_state(&state), req);
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp.into_body()).await, "The answer is 42.");

        // Verify stage marked complete
        let req = make_request("GET", "/v1/workflows/e2e-run", None);
        let resp = send!(app_from_state(&state), req);
        let body: serde_json::Value =
            serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
        assert_eq!(body["stages"].as_array().unwrap()[0]["complete"], true);
        assert_eq!(body["stages"].as_array().unwrap()[1]["complete"], false);

        // Test 2: run specific stage
        let req = make_request("POST", "/v1/workflows/e2e-run/run/01_build", None);
        let resp = send!(app_from_state(&state), req);
        assert_eq!(resp.status(), StatusCode::OK);

        assert_eq!(*call_count.lock().unwrap(), 2);

        let req = make_request("GET", "/v1/workflows/e2e-run/output/01_build", None);
        let resp = send!(app_from_state(&state), req);
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp.into_body()).await, "The answer is 42.");

        // Test 3: no pending stages after all complete
        let req = make_request("GET", "/v1/workflows/e2e-run", None);
        let resp = send!(app_from_state(&state), req);
        let body: serde_json::Value =
            serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
        assert!(body["stages"].as_array().unwrap().iter().all(|s| s["complete"] == true));

        let req = make_request("POST", "/v1/workflows/e2e-run/run", None);
        let resp = send!(app_from_state(&state), req);
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_str(&body_string(resp.into_body()).await).expect("json");
        assert!(body["message"].as_str().unwrap().contains("no pending stages"));


    }
}

mod auth {
    use super::*;

    fn make_authed_request(method: &str, uri: &str, body: Option<&str>, key: &str) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {key}"));
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        builder
            .body(body.map_or(Body::empty(), |b| Body::from(b.to_string())))
            .expect("build request")
    }

    #[tokio::test]
    async fn health_always_public() {
        let tmp = TempDir::new().expect("tempdir");
        let app = app_with_key(tmp.path(), "secret");

        let req = make_request("GET", "/v1/health", None);
        let resp = send!(app, req);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_key_returns_401() {
        let tmp = TempDir::new().expect("tempdir");
        let app = app_with_key(tmp.path(), "secret");

        let req = make_request("GET", "/v1/workflows", None);
        let resp = send!(app, req);
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_key_returns_401() {
        let tmp = TempDir::new().expect("tempdir");
        let app = app_with_key(tmp.path(), "secret");

        let req = make_authed_request("GET", "/v1/workflows", None, "wrong");
        let resp = send!(app, req);
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn valid_key_allows_access() {
        let tmp = TempDir::new().expect("tempdir");
        let app = app_with_key(tmp.path(), "secret");

        let req = make_authed_request("GET", "/v1/workflows", None, "secret");
        let resp = send!(app, req);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_key_configured_allows_all() {
        let tmp = TempDir::new().expect("tempdir");
        let app = make_app(tmp.path());

        let req = make_request("GET", "/v1/workflows", None);
        let resp = send!(app, req);
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
