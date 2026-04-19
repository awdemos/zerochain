//! LLM provider trait with OpenAI-compatible implementation.

pub mod error;
pub mod factory;
pub mod openai;
pub mod profiles;
pub mod trait_;
pub mod types;

pub use error::LLMError;
pub use factory::LLMFactory;
pub use openai::OpenAICompatibleProvider;
pub use profiles::{ProviderProfile, StageContext, resolve_profile};
pub use trait_::LLM;
pub use types::{
    CompleteResponse, Content, FinishReason, ImageUrlContent, LLMConfig, Message,
    ProviderId, Role, ThinkingMode, Tool, ToolCall, Usage,
};

pub use zerochain_core::context::MultimodalInput;
