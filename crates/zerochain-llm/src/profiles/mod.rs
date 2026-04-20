pub mod generic;
pub mod kimi_k2;

use crate::error::LLMError;
use crate::types::{CompleteResponse, LLMConfig, ThinkingMode};

#[derive(Clone, Debug, Default)]
pub struct StageContext {
    pub thinking_mode: ThinkingMode,
    pub capture_reasoning: bool,
}

pub trait ProviderProfile: Send + Sync {
    fn name(&self) -> &str;

    fn validate_config(
        &self,
        _config: &LLMConfig,
        _ctx: &StageContext,
    ) -> Result<(), LLMError> {
        Ok(())
    }

    fn augment_request(
        &self,
        extra_body: &mut serde_json::Value,
        _ctx: &StageContext,
    ) -> Result<(), LLMError> {
        let _ = extra_body;
        Ok(())
    }

    fn parse_response(
        &self,
        raw_choice: &serde_json::Value,
        parsed: &mut CompleteResponse,
        _ctx: &StageContext,
    ) {
        let _ = (raw_choice, parsed);
    }
}

#[must_use] pub fn resolve_profile(name: &str) -> Box<dyn ProviderProfile> {
    match name {
        "kimi-k2" => Box::new(kimi_k2::KimiK2Profile),
        _ => Box::new(generic::GenericProfile),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_generic_by_name() {
        let p = resolve_profile("generic");
        assert_eq!(p.name(), "generic");
    }

    #[test]
    fn resolve_kimi_k2() {
        let p = resolve_profile("kimi-k2");
        assert_eq!(p.name(), "kimi-k2");
    }

    #[test]
    fn resolve_unknown_falls_back_to_generic() {
        let p = resolve_profile("totally-unknown");
        assert_eq!(p.name(), "generic");
    }

    #[test]
    fn resolve_empty_string_is_generic() {
        let p = resolve_profile("");
        assert_eq!(p.name(), "generic");
    }

    #[test]
    fn stage_context_default() {
        let ctx = StageContext::default();
        assert!(matches!(ctx.thinking_mode, ThinkingMode::Default));
        assert!(!ctx.capture_reasoning);
    }
}
