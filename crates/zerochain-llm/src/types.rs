use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProviderId {
    OpenAI,
    OpenAICompatible {
        base_url: String,
        api_key_env: String,
    },
    LocalGGUF {
        model_path: String,
        gpu_layers: i32,
    },
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Message {
    pub role: Role,
    pub content: Content,
}

impl Message {
    pub fn new(role: Role, content: impl Into<Content>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    // Multimodal deferred to later phase
}

impl Content {
    pub fn text(&self) -> Option<&str> {
        match self {
            Content::Text(s) => Some(s),
        }
    }
}

impl From<String> for Content {
    fn from(s: String) -> Self {
        Content::Text(s)
    }
}

impl From<&str> for Content {
    fn from(s: &str) -> Self {
        Content::Text(s.to_owned())
    }
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CompleteResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub finish_reason: FinishReason,
    pub model: String,
}

#[derive(Clone, Debug)]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LLMConfig {
    pub provider: ProviderId,
    pub model: String,
    pub temperature: f32,
    pub seed: Option<u64>,
    pub max_tokens: usize,
    pub top_p: Option<f32>,
    pub context_window: usize,
}

impl CompleteResponse {
    pub fn new(content: impl Into<Option<String>>) -> Self {
        Self {
            content: content.into(),
            tool_calls: vec![],
            usage: Usage::default(),
            finish_reason: FinishReason::Stop,
            model: String::new(),
        }
    }
}

impl LLMConfig {
    /// Create an LLMConfig with sensible defaults for the given provider/model.
    pub fn new(provider: ProviderId, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
            temperature: 0.7,
            seed: None,
            max_tokens: 4096,
            top_p: None,
            context_window: 128_000,
        }
    }

    /// Derive deterministic seed from content hash (Blake3 of CID string).
    ///
    /// Sets temperature to 0.0 and top_p to 1.0 for full reproducibility.
    pub fn deterministic(mut self, content_cid: &str) -> Self {
        let hash = blake3::hash(content_cid.as_bytes());
        self.seed = Some(u64::from_le_bytes(
            hash.as_bytes()[0..8].try_into().unwrap(),
        ));
        self.temperature = 0.0;
        self.top_p = Some(1.0);
        self
    }

    pub fn is_reproducible(&self) -> bool {
        self.temperature == 0.0 && self.seed.is_some()
    }

    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = t;
        self
    }

    pub fn with_max_tokens(mut self, n: usize) -> Self {
        self.max_tokens = n;
        self
    }

    pub fn with_context_window(mut self, n: usize) -> Self {
        self.context_window = n;
        self
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_deterministic_sets_seed_and_zero_temp() {
        let config = LLMConfig::new(ProviderId::OpenAI, "gpt-4o").deterministic("bafybeigxyz123");

        assert_eq!(config.temperature, 0.0);
        assert!(config.seed.is_some());
        assert_eq!(config.top_p, Some(1.0));
        assert!(config.is_reproducible());
    }

    #[test]
    fn config_not_reproducible_by_default() {
        let config = LLMConfig::new(ProviderId::OpenAI, "gpt-4o");
        assert!(!config.is_reproducible());
    }

    #[test]
    fn deterministic_is_stable() {
        let cid = "bafybeigxyz123";
        let c1 = LLMConfig::new(ProviderId::OpenAI, "gpt-4o").deterministic(cid);
        let c2 = LLMConfig::new(ProviderId::OpenAI, "gpt-4o").deterministic(cid);
        assert_eq!(c1.seed, c2.seed);
    }

    #[test]
    fn content_text_access() {
        let c = Content::Text("hello".into());
        assert_eq!(c.text(), Some("hello"));
    }

    #[test]
    fn provider_id_serde_roundtrip() {
        let p = ProviderId::OpenAICompatible {
            base_url: "http://localhost:11434/v1".into(),
            api_key_env: "OLLAMA_KEY".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: ProviderId = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, ProviderId::OpenAICompatible { .. }));
    }
}
