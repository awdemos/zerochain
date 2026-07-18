//! Reusable tool registry with a built-in HTTP tool.
//!
//! Provides an async [`Tool`] trait, a [`ToolRegistry`] for lookup,
//! and a default [`HttpTool`] implementation for making GET/POST requests.

pub mod fs_tool;
pub mod http_tool;
pub mod registry;
pub mod shell_tool;
pub mod tool;

pub use fs_tool::{ReadFileTool, WriteFileTool};
pub use http_tool::HttpTool;
pub use registry::ToolRegistry;
pub use shell_tool::ShellTool;
pub use tool::Tool;
