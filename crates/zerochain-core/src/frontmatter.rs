use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
#[derive(Default)]
pub struct ContextFrontmatter {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub human_gate: bool,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub definition_of_done: Option<String>,
    #[serde(default)]
    pub provider_profile: Option<String>,
    #[serde(default)]
    pub thinking_mode: Option<String>,
    #[serde(default)]
    pub capture_reasoning: bool,
    #[serde(default)]
    pub multimodal_input: Vec<MultimodalInput>,
}

impl ContextFrontmatter {
    /// Merge `self` over `base`, taking child's values when present.
    #[must_use] pub fn merge(&self, base: &ContextFrontmatter) -> ContextFrontmatter {
        ContextFrontmatter {
            role: self.role.clone().or_else(|| base.role.clone()),
            container: self.container.clone().or_else(|| base.container.clone()),
            command: self.command.clone().or_else(|| base.command.clone()),
            human_gate: self.human_gate || base.human_gate,
            timeout: self.timeout.or(base.timeout),
            network: self.network.clone().or_else(|| base.network.clone()),
            definition_of_done: self.definition_of_done.clone().or_else(|| base.definition_of_done.clone()),
            provider_profile: self.provider_profile.clone().or_else(|| base.provider_profile.clone()),
            thinking_mode: self.thinking_mode.clone().or_else(|| base.thinking_mode.clone()),
            capture_reasoning: self.capture_reasoning || base.capture_reasoning,
            multimodal_input: if self.multimodal_input.is_empty() {
                base.multimodal_input.clone()
            } else {
                self.multimodal_input.clone()
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct MultimodalInput {
    #[serde(rename = "type")]
    pub input_type: String,
    pub path: String,
    #[serde(default)]
    pub detail: Option<String>,
}
