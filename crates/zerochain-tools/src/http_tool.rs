use async_trait::async_trait;
use reqwest;
use serde_json::{json, Value};
use zerochain_error::{Result, ZerochainError};

use crate::tool::Tool;

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

        let client = reqwest::Client::new();

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
                let body = response.text().await.map_err(|e| ZerochainError::Other {
                    message: format!("failed to read response body: {e}"),
                })?;

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
                let text = response.text().await.map_err(|e| ZerochainError::Other {
                    message: format!("failed to read response body: {e}"),
                })?;

                Ok(json!({ "status": status, "body": text }))
            }
            other => Err(ZerochainError::Unsupported {
                message: format!("unsupported HTTP method: {other}"),
            }),
        }
    }
}
