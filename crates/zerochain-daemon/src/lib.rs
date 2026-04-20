#![allow(clippy::missing_errors_doc)]
//! MCP server daemon for zerochain workflow execution.

pub mod container;
pub mod error;
pub mod mcp;
pub mod state;

// Re-export primary public API types so consumers don't reach into submodules.
pub use error::DaemonError;
pub use state::{AppState, InitWorkflowParams};
