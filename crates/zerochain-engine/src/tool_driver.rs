use std::path::Path;
use std::sync::Arc;
use zerochain_llm::{Tool as LlmTool, ToolCall};
use zerochain_tools::{HttpTool, ReadFileTool, ShellTool, Tool, ToolRegistry, WriteFileTool};

use crate::error::DaemonError;

/// Convert a generic `zerochain_tools::Tool` into the LLM-facing representation.
pub fn to_llm_tool(tool: &dyn Tool) -> LlmTool {
    LlmTool::new(
        tool.name().to_string(),
        tool.description().to_string(),
        tool.schema().clone(),
    )
}

/// Build the LLM tool list from the registry, filtering by the requested tool names.
/// Missing names are logged and skipped.
pub fn to_llm_tools(registry: &ToolRegistry, names: &[String]) -> Vec<LlmTool> {
    names
        .iter()
        .filter_map(|name| {
            registry
                .get(name)
                .map(|tool| to_llm_tool(tool.as_ref()))
                .or_else(|| {
                    tracing::warn!(tool_name = %name, "requested tool not found in registry");
                    None
                })
        })
        .collect()
}

pub fn default_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(HttpTool));
    registry.register(Arc::new(ReadFileTool));
    registry.register(Arc::new(WriteFileTool));
    registry.register(Arc::new(ShellTool));
    registry
}

/// Look up the tool referenced by `call` in `registry`, execute it, and return the JSON result as a string.
pub async fn execute_tool_call(
    registry: &ToolRegistry,
    call: &ToolCall,
    workspace_root: &Path,
) -> Result<String, DaemonError> {
    let tool = registry.get(&call.name).ok_or_else(|| {
        DaemonError::Workflow(zerochain_core::error::Error::PlanError {
            reason: format!("tool not found: {}", call.name),
        })
    })?;

    let mut input = call.arguments.clone();
    if matches!(call.name.as_str(), "read_file" | "write_file" | "shell") {
        input["workspace_root"] = serde_json::json!(workspace_root.to_string_lossy().to_string());
    }

    let result = tool.run(input).await.map_err(DaemonError::from)?;
    Ok(result.to_string())
}
