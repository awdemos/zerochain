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
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub tool_loop_max_iterations: Option<u32>,
    #[serde(default)]
    pub index_output: bool,
    #[serde(default)]
    pub memory_sources: Vec<String>,
    #[serde(default)]
    pub memory_chunk_size: Option<usize>,
    #[serde(default)]
    pub memory_chunk_overlap: Option<usize>,
}

impl ContextFrontmatter {
    /// Merge `self` over `base`, taking child's values when present.
    #[must_use]
    pub fn merge(&self, base: &ContextFrontmatter) -> ContextFrontmatter {
        ContextFrontmatter {
            role: self.role.clone().or_else(|| base.role.clone()),
            container: self.container.clone().or_else(|| base.container.clone()),
            command: self.command.clone().or_else(|| base.command.clone()),
            human_gate: self.human_gate || base.human_gate,
            timeout: self.timeout.or(base.timeout),
            network: self.network.clone().or_else(|| base.network.clone()),
            definition_of_done: self
                .definition_of_done
                .clone()
                .or_else(|| base.definition_of_done.clone()),
            provider_profile: self
                .provider_profile
                .clone()
                .or_else(|| base.provider_profile.clone()),
            thinking_mode: self
                .thinking_mode
                .clone()
                .or_else(|| base.thinking_mode.clone()),
            capture_reasoning: self.capture_reasoning || base.capture_reasoning,
            multimodal_input: if self.multimodal_input.is_empty() {
                base.multimodal_input.clone()
            } else {
                self.multimodal_input.clone()
            },
            tools: if self.tools.is_empty() {
                base.tools.clone()
            } else {
                self.tools.clone()
            },
            tool_loop_max_iterations: self
                .tool_loop_max_iterations
                .or(base.tool_loop_max_iterations),
            index_output: self.index_output || base.index_output,
            memory_sources: if self.memory_sources.is_empty() {
                base.memory_sources.clone()
            } else {
                self.memory_sources.clone()
            },
            memory_chunk_size: self.memory_chunk_size.or(base.memory_chunk_size),
            memory_chunk_overlap: self.memory_chunk_overlap.or(base.memory_chunk_overlap),
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

#[cfg(test)]
mod tests {
    use crate::context::Context;
    use crate::frontmatter::ContextFrontmatter;

    #[test]
    fn parse_tool_loop_max_iterations() {
        let input = "---\ntool_loop_max_iterations: 5\n---\nBody";
        let ctx = Context::parse(input).unwrap();
        assert_eq!(ctx.frontmatter.tool_loop_max_iterations, Some(5));
    }

    #[test]
    fn parse_memory_options_frontmatter() {
        let input = "---\nindex_output: true\nmemory_sources:\n  - docs/readme.md\nmemory_chunk_size: 500\nmemory_chunk_overlap: 100\n";
        let frontmatter: ContextFrontmatter = serde_yml::from_str(input).unwrap();
        assert!(frontmatter.index_output);
        assert_eq!(frontmatter.memory_sources, vec!["docs/readme.md"]);
        assert_eq!(frontmatter.memory_chunk_size, Some(500));
        assert_eq!(frontmatter.memory_chunk_overlap, Some(100));
    }
}
