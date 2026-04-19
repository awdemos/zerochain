use crate::error::LLMError;
use crate::profiles::{ProviderProfile, StageContext};
use crate::trait_::LLM;
use crate::types::{
    CompleteResponse, Content, FinishReason, LLMConfig, Message, ProviderId,
    Role, Tool, ToolCall, Usage,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, instrument, warn};

// ---------------------------------------------------------------------------
// Wire types (OpenAI /v1/chat/completions)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<serde_json::Value>,
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
    #[serde(flatten)]
    extra_body: serde_json::Value,
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
    choices: Vec<serde_json::Value>,
    usage: Option<ChatUsage>,
    model: String,
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

pub struct OpenAICompatibleProvider {
    base_url: String,
    api_key: String,
    client: Client,
}

impl OpenAICompatibleProvider {
    pub fn new(base_url: String, api_key: String) -> Result<Self, crate::error::LLMError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;
        Ok(Self {
            base_url,
            api_key,
            client,
        })
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
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("retry_after_ms")?.as_u64())
    }

    fn content_to_json(content: &Content) -> serde_json::Value {
        match content {
            Content::Text(s) => json!(s),
            Content::ImageUrl { image_url } => {
                json!({
                    "type": "image_url",
                    "image_url": {
                        "url": image_url.url,
                        "detail": image_url.detail
                    }
                })
            }
        }
    }

    fn message_to_json(msg: &Message) -> serde_json::Value {
        json!({
            "role": Self::role_str(&msg.role),
            "content": Self::content_to_json(&msg.content),
        })
    }
}

pub struct ProfiledCompleteParams<'a> {
    pub config: &'a LLMConfig,
    pub messages: &'a [Message],
    pub tools: Option<&'a [Tool]>,
    pub profile: &'a dyn ProviderProfile,
    pub stage_ctx: &'a StageContext,
}

impl OpenAICompatibleProvider {
    async fn complete_with_profile_impl(
        &self,
        params: ProfiledCompleteParams<'_>,
    ) -> Result<CompleteResponse, LLMError> {
        params.profile.validate_config(params.config, params.stage_ctx)?;

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let message_values: Vec<serde_json::Value> = params
            .messages
            .iter()
            .map(Self::message_to_json)
            .collect();

        let chat_tools = params.tools.map(|t| {
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

        let mut extra_body = serde_json::Value::Object(serde_json::Map::new());
        params.profile.augment_request(&mut extra_body, params.stage_ctx);

        let request = ChatRequest {
            model: params.config.model.clone(),
            messages: message_values.clone(),
            temperature: Some(params.config.temperature),
            seed: params.config.seed,
            max_tokens: if params.config.max_tokens > 0 {
                Some(params.config.max_tokens)
            } else {
                None
            },
            top_p: params.config.top_p,
            tools: chat_tools,
            extra_body,
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

        let raw: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| LLMError::parse(e.to_string()))?;

        let parsed_response: ChatResponse =
            serde_json::from_value(raw.clone()).map_err(|e| LLMError::parse(e.to_string()))?;

        let choice_value = parsed_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LLMError::parse("no choices in response"))?;

        let choice: ChoiceHelper =
            serde_json::from_value(choice_value.clone()).map_err(|e| LLMError::parse(e.to_string()))?;

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

        let usage = parsed_response
            .usage
            .map(|u| Usage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
            })
            .unwrap_or_default();

        let finish = Self::parse_finish_reason(choice.finish_reason.as_deref());

        let mut response = CompleteResponse {
            content: choice.message.content,
            tool_calls,
            usage,
            finish_reason: finish,
            model: parsed_response.model,
            reasoning: None,
        };

        params
            .profile
            .parse_response(&choice_value, &mut response, params.stage_ctx);

        Ok(response)
    }
}

#[derive(Deserialize)]
struct ChoiceHelper {
    message: ChoiceMessageHelper,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChoiceMessageHelper {
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

#[async_trait]
impl LLM for OpenAICompatibleProvider {
    fn provider_id(&self) -> &ProviderId {
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
        let profile = crate::profiles::resolve_profile("generic");
        let stage_ctx = StageContext::default();
        self.complete_with_profile_impl(ProfiledCompleteParams {
            config,
            messages,
            tools,
            profile: profile.as_ref(),
            stage_ctx: &stage_ctx,
        })
        .await
    }

    async fn complete_with_profile(
        &self,
        config: &LLMConfig,
        messages: &[Message],
        tools: Option<&[Tool]>,
        profile: &dyn crate::profiles::ProviderProfile,
        stage_ctx: &crate::profiles::StageContext,
    ) -> Result<CompleteResponse, LLMError> {
        self.complete_with_profile_impl(ProfiledCompleteParams {
            config,
            messages,
            tools,
            profile,
            stage_ctx,
        })
        .await
    }

    fn supports_multimodal(&self) -> bool {
        false
    }

    fn context_window(&self) -> usize {
        128_000
    }

    async fn health_check(&self) -> Result<(), LLMError> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
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

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImageUrlContent;

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

        let parsed: serde_json::Value = serde_json::from_str(raw).unwrap();
        let chat_resp: ChatResponse = serde_json::from_value(parsed).unwrap();
        assert_eq!(chat_resp.choices.len(), 1);
        assert_eq!(chat_resp.model, "gpt-4o");
    }

    #[test]
    fn content_to_json_text() {
        let c = Content::Text("hello".into());
        let j = OpenAICompatibleProvider::content_to_json(&c);
        assert_eq!(j.as_str(), Some("hello"));
    }

    #[test]
    fn content_to_json_image_url() {
        let c = Content::ImageUrl {
            image_url: ImageUrlContent {
                url: "https://example.com/img.png".into(),
                detail: Some("high".into()),
            },
        };
        let j = OpenAICompatibleProvider::content_to_json(&c);
        assert_eq!(j["type"].as_str(), Some("image_url"));
        assert_eq!(j["image_url"]["url"].as_str(), Some("https://example.com/img.png"));
        assert_eq!(j["image_url"]["detail"].as_str(), Some("high"));
    }

    #[test]
    fn message_to_json_system_text() {
        let msg = Message::new(Role::System, "you are helpful");
        let j = OpenAICompatibleProvider::message_to_json(&msg);
        assert_eq!(j["role"].as_str(), Some("system"));
        assert_eq!(j["content"].as_str(), Some("you are helpful"));
    }

    #[test]
    fn chat_request_serializes_extra_body() {
        let mut extra = serde_json::Value::Object(serde_json::Map::new());
        extra.as_object_mut().unwrap().insert(
            "thinking".to_string(),
            json!({"type": "disabled"}),
        );

        let req = ChatRequest {
            model: "test".into(),
            messages: vec![],
            temperature: Some(1.0),
            seed: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            extra_body: extra,
        };

        let serialized = serde_json::to_string(&req).unwrap();
        assert!(serialized.contains("\"thinking\""));
        assert!(serialized.contains("\"type\":\"disabled\""));
    }
}
