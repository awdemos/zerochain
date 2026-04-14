use thiserror::Error;

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
