//! Core workflow engine for zerochain.
//!
//! This crate provides the central `AppState` orchestrator, workflow registry,
//! stage execution (LLM and container), and CoW snapshot management.

pub mod container;
pub mod error;
pub mod state;

pub use error::DaemonError;
pub use state::{AppState, InitWorkflowParams, InitWorkflowRequest};
