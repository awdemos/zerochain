use crate::error::LLMError;
use crate::types::{CompleteResponse, LLMConfig, Message, ProviderId, Tool};
use async_trait::async_trait;

#[async_trait]
pub trait LLM: Send + Sync {
    fn provider_id(&self) -> &ProviderId;

    async fn complete(
        &self,
        config: &LLMConfig,
        messages: &[Message],
        tools: Option<&[Tool]>,
    ) -> Result<CompleteResponse, LLMError>;

    fn supports_multimodal(&self) -> bool;

    fn context_window(&self) -> usize;

    async fn health_check(&self) -> Result<(), LLMError>;

    fn as_any(&self) -> &dyn std::any::Any;
}
