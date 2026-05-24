use thiserror::Error;
use zerochain_error::ZerochainError;

#[derive(Error, Debug)]
pub enum LLMError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API returned error {status}: {message}")]
    Api { status: u16, message: String },

    #[error("rate limited: retry after {retry_after_ms:?}ms")]
    RateLimited { retry_after_ms: Option<u64> },

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("provider not supported: {0}")]
    UnsupportedProvider(String),

    #[error("context window exceeded: need {needed}, have {available}")]
    ContextExceeded { needed: usize, available: usize },

    #[error("tool call error: {0}")]
    ToolCall(String),

    #[error("response parsing failed: {0}")]
    Parse(String),

    #[error("{0}")]
    Other(String),
}

impl LLMError {
    pub fn api(status: u16, message: impl Into<String>) -> Self {
        Self::Api {
            status,
            message: message.into(),
        }
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::UnsupportedProvider(msg.into())
    }

    pub fn parse(msg: impl Into<String>) -> Self {
        Self::Parse(msg.into())
    }
}

impl From<LLMError> for ZerochainError {
    fn from(err: LLMError) -> Self {
        match err {
            LLMError::Http(e) => ZerochainError::Llm {
                message: format!("HTTP request failed: {e}"),
            },
            LLMError::Api { status, message } => ZerochainError::Llm {
                message: format!("API error {status}: {message}"),
            },
            LLMError::RateLimited { retry_after_ms } => {
                ZerochainError::RateLimited { retry_after_ms }
            }
            LLMError::Auth(msg) => ZerochainError::Auth { message: msg },
            LLMError::Config(msg) => ZerochainError::Configuration { message: msg },
            LLMError::UnsupportedProvider(msg) => {
                ZerochainError::Unsupported { message: msg }
            }
            LLMError::ContextExceeded { needed, available } => ZerochainError::Llm {
                message: format!("context window exceeded: need {needed}, have {available}"),
            },
            LLMError::ToolCall(msg) => ZerochainError::Llm {
                message: format!("tool call error: {msg}"),
            },
            LLMError::Parse(msg) => ZerochainError::InvalidInput { message: msg },
            LLMError::Other(msg) => ZerochainError::Other { message: msg },
        }
    }
}

impl From<ZerochainError> for LLMError {
    fn from(err: ZerochainError) -> Self {
        match err {
            ZerochainError::RateLimited { retry_after_ms } => {
                LLMError::RateLimited { retry_after_ms }
            }
            ZerochainError::Auth { message } => LLMError::Auth(message),
            ZerochainError::Configuration { message } => LLMError::Config(message),
            ZerochainError::Unsupported { message } => {
                LLMError::UnsupportedProvider(message)
            }
            ZerochainError::InvalidInput { message } => LLMError::Parse(message),
            ZerochainError::Llm { message } => LLMError::Other(message),
            other => LLMError::Other(other.to_string()),
        }
    }
}
