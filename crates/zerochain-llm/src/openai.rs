use crate::error::LLMError;
use crate::trait_::LLM;
use crate::types::{
    CompleteResponse, Content, FinishReason, LLMConfig, Message, ProviderId, Role, Tool, ToolCall,
    Usage,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

// ---------------------------------------------------------------------------
// Wire types (OpenAI /v1/chat/completions)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ChatTool>>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatTool {
    r#type: String,
    function: ChatFunction,
}

#[derive(Serialize)]
struct ChatFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Option<ChatUsage>,
    model: String,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Deserialize)]
struct ChatToolCall {
    id: String,
    r#function: ChatFunctionCall,
}

#[derive(Deserialize)]
struct ChatFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct ChatUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    error: Option<ApiErrorDetail>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct ApiErrorDetail {
    message: String,
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// OpenAI-compatible provider (works with OpenAI, Moonshot/Kimi, Zhipu/GLM,
/// Ollama, vLLM, or any server exposing `/v1/chat/completions`).
pub struct OpenAICompatibleProvider {
    base_url: String,
    api_key: String,
    client: Client,
}

impl OpenAICompatibleProvider {
    pub fn new(base_url: String, api_key: String, _model: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("failed to build reqwest client");
        Self {
            base_url,
            api_key,
            client,
        }
    }

    fn role_str(role: &Role) -> &'static str {
        match role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }

    fn parse_finish_reason(raw: Option<&str>) -> FinishReason {
        match raw {
            Some("stop") => FinishReason::Stop,
            Some("length") => FinishReason::Length,
            Some("tool_calls") => FinishReason::ToolCalls,
            Some("content_filter") => FinishReason::ContentFilter,
            _ => {
                warn!(reason = ?raw, "unknown finish_reason, defaulting to Stop");
                FinishReason::Stop
            }
        }
    }

    fn map_status_error(status: u16, body: &str) -> LLMError {
        if status == 401 || status == 403 {
            let msg = Self::extract_error_message(body).unwrap_or_else(|| "authentication failed".into());
            return LLMError::Auth(msg);
        }
        if status == 429 {
            let retry = Self::parse_retry_after(body);
            return LLMError::RateLimited {
                retry_after_ms: retry,
            };
        }
        let msg = Self::extract_error_message(body).unwrap_or_else(|| body.into());
        LLMError::api(status, msg)
    }

    fn extract_error_message(body: &str) -> Option<String> {
        serde_json::from_str::<ApiErrorBody>(body)
            .ok()
            .and_then(|b| {
                b.error
                    .map(|e| e.message)
                    .or(b.message)
            })
    }

    fn parse_retry_after(body: &str) -> Option<u64> {
        // Some providers include retry-after-ms or similar hints; best-effort.
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("retry_after_ms")?.as_u64())
    }
}

#[async_trait]
impl LLM for OpenAICompatibleProvider {
    fn provider_id(&self) -> &ProviderId {
        // We return a transient ProviderId; the canonical one lives in LLMConfig.
        // This is fine for identification purposes.
        static OPENAI: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        OPENAI.get_or_init(|| ProviderId::OpenAI)
    }

    #[instrument(skip(self, messages, tools), fields(model = %config.model))]
    async fn complete(
        &self,
        config: &LLMConfig,
        messages: &[Message],
        tools: Option<&[Tool]>,
    ) -> Result<CompleteResponse, LLMError> {
        let url = format!("{}/v1/chat/completions", self.base_url.trim_end_matches('/'));

        let chat_messages: Vec<ChatMessage> = messages
            .iter()
            .map(|m| ChatMessage {
                role: Self::role_str(&m.role).to_owned(),
                content: match &m.content {
                    Content::Text(s) => s.clone(),
                },
            })
            .collect();

        let chat_tools = tools.map(|t| {
            t.iter()
                .map(|tool| ChatTool {
                    r#type: "function".into(),
                    function: ChatFunction {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        parameters: tool.parameters.clone(),
                    },
                })
                .collect::<Vec<_>>()
        });

        let request = ChatRequest {
            model: config.model.clone(),
            messages: chat_messages,
            temperature: Some(config.temperature),
            seed: config.seed,
            max_tokens: if config.max_tokens > 0 {
                Some(config.max_tokens)
            } else {
                None
            },
            top_p: config.top_p,
            tools: chat_tools,
        };

        debug!(url = %url, "sending chat completion request");

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;

        if !status.is_success() {
            return Err(Self::map_status_error(status.as_u16(), &body));
        }

        let parsed: ChatResponse =
            serde_json::from_str(&body).map_err(|e| LLMError::parse(e.to_string()))?;

        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LLMError::parse("no choices in response"))?;

        let tool_calls: Vec<ToolCall> = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| {
                let args = serde_json::from_str(&tc.function.arguments).unwrap_or_else(|e| {
                    warn!(error = %e, "failed to parse tool call arguments as JSON");
                    serde_json::Value::Null
                });
                ToolCall {
                    id: tc.id,
                    name: tc.function.name,
                    arguments: args,
                }
            })
            .collect();

        let usage = parsed
            .usage
            .map(|u| Usage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
            })
            .unwrap_or_default();

        let finish = Self::parse_finish_reason(choice.finish_reason.as_deref());

        Ok(CompleteResponse {
            content: choice.message.content,
            tool_calls,
            usage,
            finish_reason: finish,
            model: parsed.model,
        })
    }

    fn supports_multimodal(&self) -> bool {
        false
    }

    fn context_window(&self) -> usize {
        128_000
    }

    async fn health_check(&self) -> Result<(), LLMError> {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(Self::map_status_error(status, &body))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_finish_reason_known_variants() {
        assert!(matches!(
            OpenAICompatibleProvider::parse_finish_reason(Some("stop")),
            FinishReason::Stop
        ));
        assert!(matches!(
            OpenAICompatibleProvider::parse_finish_reason(Some("length")),
            FinishReason::Length
        ));
        assert!(matches!(
            OpenAICompatibleProvider::parse_finish_reason(Some("tool_calls")),
            FinishReason::ToolCalls
        ));
        assert!(matches!(
            OpenAICompatibleProvider::parse_finish_reason(Some("content_filter")),
            FinishReason::ContentFilter
        ));
    }

    #[test]
    fn parse_finish_reason_unknown_defaults_to_stop() {
        assert!(matches!(
            OpenAICompatibleProvider::parse_finish_reason(Some("unknown")),
            FinishReason::Stop
        ));
        assert!(matches!(
            OpenAICompatibleProvider::parse_finish_reason(None),
            FinishReason::Stop
        ));
    }

    #[test]
    fn map_status_error_auth() {
        let err = OpenAICompatibleProvider::map_status_error(
            401,
            r#"{"error":{"message":"invalid api key"}}"#,
        );
        assert!(matches!(err, LLMError::Auth(_)));
    }

    #[test]
    fn map_status_error_rate_limit() {
        let err = OpenAICompatibleProvider::map_status_error(
            429,
            r#"{"retry_after_ms":5000}"#,
        );
        assert!(matches!(err, LLMError::RateLimited { .. }));
    }

    #[test]
    fn map_status_error_generic_api() {
        let err = OpenAICompatibleProvider::map_status_error(
            500,
            r#"{"error":{"message":"internal error"}}"#,
        );
        assert!(matches!(err, LLMError::Api { status: 500, .. }));
    }

    #[test]
    fn map_status_error_generic_no_json() {
        let err = OpenAICompatibleProvider::map_status_error(502, "bad gateway");
        assert!(matches!(err, LLMError::Api { status: 502, .. }));
    }

    #[test]
    fn response_json_parsing() {
        let raw = r#"{
            "choices": [{
                "message": {
                    "content": "Hello!",
                    "tool_calls": null
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5},
            "model": "gpt-4o"
        }"#;

        let parsed: ChatResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.choices.len(), 1);
        assert_eq!(parsed.choices[0].message.content.as_deref(), Some("Hello!"));
        assert_eq!(parsed.model, "gpt-4o");
        let usage = parsed.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
    }

    #[test]
    fn response_with_tool_calls() {
        let raw = r#"{
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_123",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\": \"/tmp/x\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 10},
            "model": "gpt-4o"
        }"#;

        let parsed: ChatResponse = serde_json::from_str(raw).unwrap();
        let tc = &parsed.choices[0].message.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.id, "call_123");
        assert_eq!(tc.function.name, "read_file");
    }
}
