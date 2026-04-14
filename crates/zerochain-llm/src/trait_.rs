use crate::error::LLMError;
use crate::types::{CompleteResponse, LLMConfig, Message, ProviderId, Tool};
use async_trait::async_trait;

/// Provider-agnostic interface for LLM completion.
#[async_trait]
pub trait LLM: Send + Sync {
    /// Identifier of the backing provider.
    fn provider_id(&self) -> &ProviderId;

    /// Send a chat-completion request.
    ///
    /// `tools` is optional; when `Some`, the model may respond with tool calls
    /// inside the [`CompleteResponse`].
    async fn complete(
        &self,
        config: &LLMConfig,
        messages: &[Message],
        tools: Option<&[Tool]>,
    ) -> Result<CompleteResponse, LLMError>;

    /// Whether the provider can handle multimodal content (images, etc.).
    fn supports_multimodal(&self) -> bool;

    /// Maximum context window size in tokens for the configured model.
    fn context_window(&self) -> usize;

    /// Lightweight connectivity / auth check.
    async fn health_check(&self) -> Result<(), LLMError>;
}
