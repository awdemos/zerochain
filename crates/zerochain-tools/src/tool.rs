use async_trait::async_trait;
use serde_json::Value;
use zerochain_error::Result;

/// An async tool that can be registered and invoked by name.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name used for registry lookup.
    fn name(&self) -> &str;

    /// Human-readable description of what the tool does.
    fn description(&self) -> &str;

    /// JSON Schema describing the expected input shape.
    fn schema(&self) -> Value;

    /// Execute the tool with the provided JSON input.
    async fn run(&self, input: Value) -> Result<Value>;
}
