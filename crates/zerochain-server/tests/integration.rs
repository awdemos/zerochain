use std::path::Path;

use axum::body::Body;
use http::Request;
use http::StatusCode;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use zerochain_server::routes;
use zerochain_server::state::ServerState;

fn make_app(workspace: &Path) -> axum::Router {
    let state = ServerState::new(workspace);
    routes::routes(state)
}

fn app_from_state(state: &ServerState) -> axum::Router {
    routes::routes(state.clone())
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
        .body(body.map(|b| Body::from(b.to_string())).unwrap_or(Body::empty()))
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
