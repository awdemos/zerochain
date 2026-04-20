use crate::error::LLMError;
use crate::profiles::{ProviderProfile, StageContext};
use crate::types::{CompleteResponse, LLMConfig};

pub struct GenericProfile;

impl ProviderProfile for GenericProfile {
    fn name(&self) -> &'static str {
        "generic"
    }

    fn validate_config(
        &self,
        _config: &LLMConfig,
        _ctx: &StageContext,
    ) -> Result<(), LLMError> {
        Ok(())
    }

    fn augment_request(
        &self,
        _extra_body: &mut serde_json::Value,
        _ctx: &StageContext,
    ) {
    }

    fn parse_response(
        &self,
        _raw_choice: &serde_json::Value,
        _parsed: &mut CompleteResponse,
        _ctx: &StageContext,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_name() {
        assert_eq!(GenericProfile.name(), "generic");
    }

    #[test]
    fn generic_validate_always_ok() {
        let config = LLMConfig::new(crate::types::ProviderId::OpenAI, "gpt-4o");
        let ctx = StageContext::default();
        let p = GenericProfile;
        assert!(p.validate_config(&config, &ctx).is_ok());
    }

    #[test]
    fn generic_augment_is_noop() {
        let p = GenericProfile;
        let mut extra = serde_json::Value::Object(serde_json::Map::new());
        let ctx = StageContext::default();
        p.augment_request(&mut extra, &ctx);
        assert!(extra.as_object().unwrap().is_empty());
    }

    #[test]
    fn generic_parse_is_noop() {
        let p = GenericProfile;
        let raw = serde_json::json!({"message": {"reasoning_content": "should be ignored"}});
        let mut response = CompleteResponse::new(String::from("hello"));
        let ctx = StageContext {
            thinking_mode: crate::types::ThinkingMode::Default,
            capture_reasoning: true,
        };
        p.parse_response(&raw, &mut response, &ctx);
        assert!(response.reasoning.is_none());
    }
}
