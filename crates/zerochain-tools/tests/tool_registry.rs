use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zerochain_tools::{HttpTool, Tool, ToolRegistry};

#[tokio::test]
async fn registry_default_includes_http_tool() {
    let registry = ToolRegistry::default();
    let tool = registry
        .get("http")
        .expect("http tool should be registered by default");
    assert_eq!(tool.name(), "http");
    assert!(!tool.description().is_empty());
    assert_eq!(
        tool.schema().get("type").unwrap().as_str().unwrap(),
        "object"
    );
}

#[tokio::test]
async fn http_tool_gets_local_server() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let n = socket.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);
        assert!(request.starts_with("GET /"));

        let body = r#"{"ok":true}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });

    let tool = HttpTool;
    let input = serde_json::json!({
        "url": format!("http://{}/get", addr),
        "method": "GET"
    });
    let result = tool.run(input).await.unwrap();

    assert_eq!(result.get("status").unwrap().as_u64().unwrap(), 200);
    assert!(result
        .get("body")
        .unwrap()
        .as_str()
        .unwrap()
        .contains(r#"{"ok":true}"#));

    server.await.unwrap();
}

#[tokio::test]
async fn http_tool_posts_local_server() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let n = socket.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);
        assert!(request.starts_with("POST /post"));
        assert!(request.contains(r#"{"hello":"world"}"#));

        let body = r#"{"posted":true}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });

    let tool = HttpTool;
    let input = serde_json::json!({
        "url": format!("http://{}/post", addr),
        "method": "POST",
        "body": {"hello": "world"}
    });
    let result = tool.run(input).await.unwrap();

    assert_eq!(result.get("status").unwrap().as_u64().unwrap(), 200);
    assert!(result
        .get("body")
        .unwrap()
        .as_str()
        .unwrap()
        .contains(r#"{"posted":true}"#));

    server.await.unwrap();
}

#[tokio::test]
async fn registry_runs_http_tool_by_name() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let n = socket.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);
        assert!(request.starts_with("GET /"));

        let body = r#"{"by_name":true}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });

    let registry = ToolRegistry::default();
    let tool = registry.get("http").unwrap();
    let input = serde_json::json!({
        "url": format!("http://{}/by-name", addr),
        "method": "GET"
    });
    let result = tool.run(input).await.unwrap();

    assert_eq!(result.get("status").unwrap().as_u64().unwrap(), 200);
    assert!(result
        .get("body")
        .unwrap()
        .as_str()
        .unwrap()
        .contains(r#"{"by_name":true}"#));

    server.await.unwrap();
}
