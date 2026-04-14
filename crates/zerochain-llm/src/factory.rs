use crate::error::LLMError;
use crate::openai::OpenAICompatibleProvider;
use crate::trait_::LLM;
use crate::types::LLMConfig;

/// Instantiate a concrete [`LLM`] provider from an [`LLMConfig`].
pub struct LLMFactory;

impl LLMFactory {
    /// Create a boxed [`LLM`] matching the provider described in `config`.
    ///
    /// For `ProviderId::OpenAI` and `ProviderId::OpenAICompatible`, the API key
    /// is read from the environment at call time. For `LocalGGUF`, this returns
    /// an error (not yet implemented).
    pub fn create(config: &LLMConfig) -> Result<Box<dyn LLM>, LLMError> {
        match &config.provider {
            crate::types::ProviderId::OpenAI => {
                let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
                    LLMError::Config("OPENAI_API_KEY environment variable not set".into())
                })?;
                Ok(Box::new(OpenAICompatibleProvider::new(
                    "https://api.openai.com".into(),
                    api_key,
                    config.model.clone(),
                )))
            }
            crate::types::ProviderId::OpenAICompatible {
                base_url,
                api_key_env,
            } => {
                let api_key = std::env::var(api_key_env).map_err(|_| {
                    LLMError::Config(format!("environment variable `{api_key_env}` not set"))
                })?;
                Ok(Box::new(OpenAICompatibleProvider::new(
                    base_url.clone(),
                    api_key,
                    config.model.clone(),
                )))
            }
            crate::types::ProviderId::LocalGGUF { .. } => Err(LLMError::unsupported(
                "LocalGGUF provider not yet implemented",
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProviderId;

    #[test]
    fn localgguf_returns_unsupported() {
        let config = LLMConfig::new(
            ProviderId::LocalGGUF {
                model_path: "/dev/null".into(),
                gpu_layers: 0,
            },
            "test",
        );
        match LLMFactory::create(&config) {
            Err(LLMError::UnsupportedProvider(_)) => {}
            Ok(_) => panic!("expected error, got success"),
            Err(e) => panic!("expected UnsupportedProvider, got {e}"),
        }
    }

    #[test]
    fn openai_without_env_returns_config_error() {
        std::env::remove_var("OPENAI_API_KEY");
        let config = LLMConfig::new(ProviderId::OpenAI, "gpt-4o");
        match LLMFactory::create(&config) {
            Err(LLMError::Config(_)) => {}
            Ok(_) => panic!("expected error, got success"),
            Err(e) => panic!("expected Config error, got {e}"),
        }
    }

    #[test]
    fn compatible_without_env_returns_config_error() {
        std::env::remove_var("MY_TEST_KEY");
        let config = LLMConfig::new(
            ProviderId::OpenAICompatible {
                base_url: "http://localhost:11434".into(),
                api_key_env: "MY_TEST_KEY".into(),
            },
            "llama3",
        );
        match LLMFactory::create(&config) {
            Err(LLMError::Config(_)) => {}
            Ok(_) => panic!("expected error, got success"),
            Err(e) => panic!("expected Config error, got {e}"),
        }
    }
}
