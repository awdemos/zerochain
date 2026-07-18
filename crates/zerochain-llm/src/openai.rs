use crate::error::LLMError;
use crate::profiles::{ProviderProfile, StageContext};
use crate::trait_::LLM;
use crate::types::{
    CompleteResponse, FinishReason, LLMConfig, Message, ProviderId, Tool, ToolCall, Usage,
};
#[cfg(test)]
use crate::types::{Content, Role};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
use tracing::{debug, instrument, warn};

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
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
    #[serde(default)]
    usage: Option<ChatUsage>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: usize,
    #[serde(default)]
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

pub struct OpenAICompatibleProvider {
    provider_id: crate::types::ProviderId,
    base_url: String,
    api_key: String,
    client: Client,
}

impl OpenAICompatibleProvider {
    /// # Errors
    ///
    /// Returns `LLMError` if the HTTP client cannot be built.
    pub fn new(
        provider_id: crate::types::ProviderId,
        base_url: String,
        api_key: String,
    ) -> Result<Self, crate::error::LLMError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;
        Ok(Self {
            provider_id,
            base_url,
            api_key,
            client,
        })
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
            let msg =
                Self::extract_error_message(body).unwrap_or_else(|| "authentication failed".into());
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
            .and_then(|b| b.error.map(|e| e.message).or(b.message))
    }

    fn parse_retry_after(body: &str) -> Option<u64> {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("retry_after_ms")?.as_u64())
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
        params
            .profile
            .validate_config(params.config, params.stage_ctx)?;

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

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
        params
            .profile
            .augment_request(&mut extra_body, params.stage_ctx)?;

        let request = ChatRequest {
            model: params.config.model.clone(),
            messages: params.messages.to_vec(),
            temperature: Some(params.config.temperature),
            seed: params.config.seed,
            max_tokens: if params.config.max_tokens > 0 {
                Some(params.config.max_tokens)
            } else {
                None
            },
            top_p: params.config.top_p,
            stream: Some(false),
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

        let choice: ChoiceHelper = serde_json::from_value(choice_value.clone())
            .map_err(|e| LLMError::parse(e.to_string()))?;

        let tool_calls: Vec<ToolCall> = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| {
                let function = tc.function.ok_or_else(|| {
                    LLMError::parse("tool call missing 'function' field".to_string())
                })?;
                let id = tc
                    .id
                    .ok_or_else(|| LLMError::parse("tool call missing 'id' field".to_string()))?;
                let name = function.name.ok_or_else(|| {
                    LLMError::parse("tool call function missing 'name' field".to_string())
                })?;
                let arguments = function.arguments.ok_or_else(|| {
                    LLMError::parse("tool call function missing 'arguments' field".to_string())
                })?;
                let args = serde_json::from_str(&arguments).map_err(|e| {
                    LLMError::parse(format!("failed to parse tool call arguments as JSON: {e}"))
                })?;
                Ok(ToolCall {
                    id,
                    name,
                    arguments: args,
                })
            })
            .collect::<Result<Vec<_>, LLMError>>()?;

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
            model: parsed_response
                .model
                .unwrap_or_else(|| params.config.model.clone()),
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
    #[serde(default)]
    id: Option<String>,
    #[serde(default, rename = "function")]
    function: Option<ChatFunctionCall>,
}

#[derive(Deserialize)]
struct ChatFunctionCall {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[async_trait]
impl LLM for OpenAICompatibleProvider {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    #[instrument(skip(self, messages, tools), fields(model = %config.model))]
    async fn complete(
        &self,
        config: &LLMConfig,
        messages: &[Message],
        tools: Option<&[Tool]>,
    ) -> Result<CompleteResponse, LLMError> {
        let profile_name = crate::profiles::profile_name_for_model(&config.model);
        let profile = crate::profiles::resolve_profile(profile_name);
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
        true
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
        let err = OpenAICompatibleProvider::map_status_error(429, r#"{"retry_after_ms":5000}"#);
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
        assert_eq!(chat_resp.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn content_serializes_text() {
        let c = Content::Text("hello".into());
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j.as_str(), Some("hello"));
    }

    #[test]
    fn content_serializes_image_url() {
        let c = Content::ImageUrl {
            image_url: ImageUrlContent {
                url: "https://example.com/img.png".into(),
                detail: Some("high".into()),
            },
        };
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["type"].as_str(), Some("image_url"));
        assert_eq!(
            j["image_url"]["url"].as_str(),
            Some("https://example.com/img.png")
        );
        assert_eq!(j["image_url"]["detail"].as_str(), Some("high"));
    }

    #[test]
    fn message_serializes_system_text() {
        let msg = Message::new(Role::System, "you are helpful");
        let j = serde_json::to_value(&msg).unwrap();
        assert_eq!(j["role"].as_str(), Some("system"));
        assert_eq!(j["content"].as_str(), Some("you are helpful"));
    }

    #[test]
    fn chat_request_serializes_extra_body() {
        let mut extra = serde_json::Value::Object(serde_json::Map::new());
        extra
            .as_object_mut()
            .unwrap()
            .insert("thinking".to_string(), json!({"type": "disabled"}));

        let req = ChatRequest {
            model: "test".into(),
            messages: vec![],
            temperature: Some(1.0),
            seed: None,
            max_tokens: None,
            top_p: None,
            stream: None,
            tools: None,
            extra_body: extra,
        };

        let serialized = serde_json::to_string(&req).unwrap();
        assert!(serialized.contains("\"thinking\""));
        assert!(serialized.contains("\"type\":\"disabled\""));
    }

    #[test]
    fn chat_request_sets_stream_false() {
        let req = ChatRequest {
            model: "test".into(),
            messages: vec![],
            temperature: None,
            seed: None,
            max_tokens: None,
            top_p: None,
            stream: Some(false),
            tools: None,
            extra_body: serde_json::Value::Object(serde_json::Map::new()),
        };
        let serialized = serde_json::to_string(&req).unwrap();
        assert!(serialized.contains("\"stream\":false"));
    }
}
