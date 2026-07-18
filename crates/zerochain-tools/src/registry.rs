use std::collections::HashMap;
use std::sync::Arc;

use crate::fs_tool::{ReadFileTool, WriteFileTool};
use crate::http_tool::HttpTool;
use crate::shell_tool::ShellTool;
use crate::tool::Tool;

/// Registry that maps tool names to their implementations.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool, keyed by its name.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
}

impl Default for ToolRegistry {
    /// Build a registry with the default built-in tools.
    fn default() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(HttpTool));
        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(WriteFileTool));
        registry.register(Arc::new(ShellTool));
        registry
    }
}
