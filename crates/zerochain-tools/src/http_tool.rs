use async_trait::async_trait;
use reqwest;
use serde_json::{json, Value};
use std::time::Duration;
use zerochain_error::{Result, ZerochainError};

use crate::tool::Tool;

/// Total wall-clock limit for one HTTP request, including body download.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Limit for establishing the TCP connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum response body size accepted, in bytes.
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

fn build_client(timeout: Duration, connect_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(connect_timeout)
        .build()
        .expect("reqwest client with default configuration must build")
}

/// Reads a response body to completion, refusing bodies larger than
/// [`MAX_BODY_BYTES`]. Uses chunked reads so a hostile or broken server cannot
/// make us buffer an unbounded body.
async fn read_body_capped(response: reqwest::Response) -> Result<String> {
    let mut response = response;
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| ZerochainError::Other {
        message: format!("failed to read response body: {e}"),
    })? {
        if bytes.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(ZerochainError::Other {
                message: format!("response body exceeds {MAX_BODY_BYTES} byte limit"),
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Built-in tool that performs HTTP GET or POST requests.
#[derive(Clone, Copy, Debug, Default)]
pub struct HttpTool;

#[async_trait]
impl Tool for HttpTool {
    fn name(&self) -> &str {
        "http"
    }

    fn description(&self) -> &str {
        "Makes HTTP GET or POST requests and returns the response status and body."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to request."
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST"]
                },
                "body": {
                    "type": "object",
                    "description": "Optional JSON body for POST requests."
                }
            },
            "required": ["url", "method"]
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let url = input.get("url").and_then(Value::as_str).ok_or_else(|| {
            ZerochainError::InvalidInput {
                message: "missing 'url' field".to_string(),
            }
        })?;

        let method = input.get("method").and_then(Value::as_str).ok_or_else(|| {
            ZerochainError::InvalidInput {
                message: "missing 'method' field".to_string(),
            }
        })?;

        let client = build_client(REQUEST_TIMEOUT, CONNECT_TIMEOUT);

        match method.to_ascii_uppercase().as_str() {
            "GET" => {
                let response = client
                    .get(url)
                    .send()
                    .await
                    .map_err(|e| ZerochainError::Other {
                        message: format!("HTTP GET request failed: {e}"),
                    })?;

                let status = response.status().as_u16();
                let body = read_body_capped(response).await?;

                Ok(json!({ "status": status, "body": body }))
            }
            "POST" => {
                let body = input.get("body").cloned().unwrap_or_else(|| json!({}));
                let response = client.post(url).json(&body).send().await.map_err(|e| {
                    ZerochainError::Other {
                        message: format!("HTTP POST request failed: {e}"),
                    }
                })?;

                let status = response.status().as_u16();
                let text = read_body_capped(response).await?;

                Ok(json!({ "status": status, "body": text }))
            }
            other => Err(ZerochainError::Unsupported {
                message: format!("unsupported HTTP method: {other}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn get_rejects_oversized_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await;

            let body = vec![b'a'; MAX_BODY_BYTES + 1];
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(headers.as_bytes()).await;
            let _ = socket.write_all(&body).await;
        });

        let tool = HttpTool;
        let err = tool
            .run(json!({
                "url": format!("http://{}/big", addr),
                "method": "GET"
            }))
            .await
            .expect_err("oversized body must be rejected");
        assert!(
            matches!(err, ZerochainError::Other { .. }),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string().contains("byte limit"),
            "unexpected error: {err}"
        );

        server.await.unwrap();
    }

    #[tokio::test]
    async fn client_times_out_against_stalling_server() {
        // The tool itself is pinned to REQUEST_TIMEOUT; exercise the same
        // builder with a short timeout so the test does not sleep a minute.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            // Accept the connection but never respond.
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        let client = build_client(Duration::from_millis(100), Duration::from_millis(100));
        let err = client
            .get(format!("http://{}/stall", addr))
            .send()
            .await
            .expect_err("stalling server must trip the timeout");
        assert!(err.is_timeout(), "expected timeout error, got: {err}");

        server.abort();
    }
}
