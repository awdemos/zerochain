//! MCP server daemon for zerochain workflow execution.

pub mod mcp;

// Re-export primary public API types from the engine crate so existing
// consumers don't break immediately.
pub use zerochain_engine::{DaemonError, AppState, InitWorkflowParams, InitWorkflowRequest};
