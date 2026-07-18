//! Reusable tool registry with a built-in HTTP tool.
//!
//! Provides an async [`Tool`] trait, a [`ToolRegistry`] for lookup,
//! and a default [`HttpTool`] implementation for making GET/POST requests.

pub mod http_tool;
pub mod registry;
pub mod tool;

pub use http_tool::HttpTool;
pub use registry::ToolRegistry;
pub use tool::Tool;
