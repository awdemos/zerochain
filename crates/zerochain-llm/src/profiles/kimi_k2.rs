use crate::error::LLMError;
use crate::profiles::{ProviderProfile, StageContext};
use crate::types::{CompleteResponse, LLMConfig, ThinkingMode};

pub struct KimiK2Profile;

impl ProviderProfile for KimiK2Profile {
    fn name(&self) -> &str {
        "kimi-k2"
    }

    fn validate_config(
        &self,
        config: &LLMConfig,
        ctx: &StageContext,
    ) -> Result<(), LLMError> {
        if !matches!(ctx.thinking_mode, ThinkingMode::Disabled)
            && (config.temperature - 1.0).abs() > f32::EPSILON {
                return Err(LLMError::Config(
                    "Kimi K2.5 thinking mode requires temperature=1.0".into(),
                ));
            }
        Ok(())
    }

    fn augment_request(
        &self,
        extra_body: &mut serde_json::Value,
        ctx: &StageContext,
    ) {
        match &ctx.thinking_mode {
            ThinkingMode::Disabled => {
                extra_body.as_object_mut().unwrap().insert(
                    "thinking".to_string(),
                    serde_json::json!({"type": "disabled"}),
                );
            }
            ThinkingMode::Extended { budget_tokens } => {
                extra_body.as_object_mut().unwrap().insert(
                    "thinking".to_string(),
                    serde_json::json!({
                        "type": "enabled",
                        "budget_tokens": budget_tokens
                    }),
                );
            }
            ThinkingMode::Default => {}
        }
    }

    fn parse_response(
        &self,
        raw_choice: &serde_json::Value,
        parsed: &mut CompleteResponse,
        ctx: &StageContext,
    ) {
        if ctx.capture_reasoning {
            if let Some(reasoning) = raw_choice
                .get("message")
                .and_then(|m| m.get("reasoning_content"))
                .and_then(|r| r.as_str())
            {
                parsed.reasoning = Some(reasoning.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProviderId;

    fn profile() -> KimiK2Profile {
        KimiK2Profile
    }

    fn default_config() -> LLMConfig {
        LLMConfig::new(ProviderId::OpenAI, "kimi-k2.5")
    }

    #[test]
    fn kimi_k2_name() {
        assert_eq!(profile().name(), "kimi-k2");
    }

    #[test]
    fn validate_ok_when_thinking_default_and_temp_1() {
        let config = default_config().with_temperature(1.0);
        let ctx = StageContext {
            thinking_mode: ThinkingMode::Default,
            capture_reasoning: false,
        };
        assert!(profile().validate_config(&config, &ctx).is_ok());
    }

    #[test]
    fn validate_ok_when_thinking_disabled_any_temp() {
        let config = default_config().with_temperature(0.5);
        let ctx = StageContext {
            thinking_mode: ThinkingMode::Disabled,
            capture_reasoning: false,
        };
        assert!(profile().validate_config(&config, &ctx).is_ok());
    }

    #[test]
    fn validate_rejects_thinking_default_with_wrong_temp() {
        let config = default_config().with_temperature(0.7);
        let ctx = StageContext {
            thinking_mode: ThinkingMode::Default,
            capture_reasoning: false,
        };
        let err = profile().validate_config(&config, &ctx);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("temperature=1.0"), "got: {msg}");
    }

    #[test]
    fn validate_rejects_thinking_extended_with_wrong_temp() {
        let config = default_config().with_temperature(0.7);
        let ctx = StageContext {
            thinking_mode: ThinkingMode::Extended { budget_tokens: 8192 },
            capture_reasoning: false,
        };
        assert!(profile().validate_config(&config, &ctx).is_err());
    }

    #[test]
    fn validate_ok_thinking_extended_with_temp_1() {
        let config = default_config().with_temperature(1.0);
        let ctx = StageContext {
            thinking_mode: ThinkingMode::Extended { budget_tokens: 16384 },
            capture_reasoning: false,
        };
        assert!(profile().validate_config(&config, &ctx).is_ok());
    }

    #[test]
    fn augment_thinking_disabled() {
        let mut extra = serde_json::Value::Object(serde_json::Map::new());
        let ctx = StageContext {
            thinking_mode: ThinkingMode::Disabled,
            capture_reasoning: false,
        };
        profile().augment_request(&mut extra, &ctx);
        assert_eq!(extra["thinking"]["type"].as_str(), Some("disabled"));
    }

    #[test]
    fn augment_thinking_extended() {
        let mut extra = serde_json::Value::Object(serde_json::Map::new());
        let ctx = StageContext {
            thinking_mode: ThinkingMode::Extended { budget_tokens: 4096 },
            capture_reasoning: false,
        };
        profile().augment_request(&mut extra, &ctx);
        assert_eq!(extra["thinking"]["type"].as_str(), Some("enabled"));
        assert_eq!(extra["thinking"]["budget_tokens"].as_u64(), Some(4096));
    }

    #[test]
    fn augment_thinking_default_is_noop() {
        let mut extra = serde_json::Value::Object(serde_json::Map::new());
        let ctx = StageContext {
            thinking_mode: ThinkingMode::Default,
            capture_reasoning: false,
        };
        profile().augment_request(&mut extra, &ctx);
        assert!(extra.as_object().unwrap().is_empty());
    }

    #[test]
    fn parse_captures_reasoning_when_enabled() {
        let raw = serde_json::json!({
            "message": {
                "content": "result",
                "reasoning_content": "I thought about this carefully"
            }
        });
        let mut response = CompleteResponse::new(String::from("result"));
        let ctx = StageContext {
            thinking_mode: ThinkingMode::Default,
            capture_reasoning: true,
        };
        profile().parse_response(&raw, &mut response, &ctx);
        assert_eq!(response.reasoning.as_deref(), Some("I thought about this carefully"));
    }

    #[test]
    fn parse_skips_reasoning_when_disabled() {
        let raw = serde_json::json!({
            "message": {
                "content": "result",
                "reasoning_content": "should be ignored"
            }
        });
        let mut response = CompleteResponse::new(String::from("result"));
        let ctx = StageContext {
            thinking_mode: ThinkingMode::Default,
            capture_reasoning: false,
        };
        profile().parse_response(&raw, &mut response, &ctx);
        assert!(response.reasoning.is_none());
    }

    #[test]
    fn parse_handles_missing_reasoning_field() {
        let raw = serde_json::json!({
            "message": {
                "content": "result"
            }
        });
        let mut response = CompleteResponse::new(String::from("result"));
        let ctx = StageContext {
            thinking_mode: ThinkingMode::Default,
            capture_reasoning: true,
        };
        profile().parse_response(&raw, &mut response, &ctx);
        assert!(response.reasoning.is_none());
    }
}
