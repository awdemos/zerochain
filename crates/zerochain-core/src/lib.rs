#![allow(clippy::missing_errors_doc)]
//! Core workflow model: stages, CONTEXT.md parsing, execution DAG.

pub mod context;
pub mod error;
pub mod frontmatter;
pub mod jj;
pub mod lua_engine;
pub mod plan;
pub mod stage;
pub mod task;
pub mod template;
pub mod workflow;
