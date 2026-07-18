//! Reusable tool registry with built-in HTTP and file tools.
//!
//! Provides an async [`Tool`] trait, a [`ToolRegistry`] for lookup,
//! and default [`HttpTool`], [`ReadFileTool`], and [`WriteFileTool`]
//! implementations.

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
