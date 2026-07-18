//! Core workflow engine for zerochain.
//!
//! This crate provides the central `AppState` orchestrator, workflow registry,
//! stage execution (LLM and container), and CoW snapshot management.

pub mod actor;
pub mod container;
pub mod error;
pub mod llm_driver;
pub mod registry;
pub mod state;
mod tool_driver;

pub use actor::{ActorMessage, WorkflowActor, WorkflowHandle};
pub use error::DaemonError;
pub use registry::WorkflowRegistry;
pub use state::{AppState, InitWorkflowParams, InitWorkflowRequest};
