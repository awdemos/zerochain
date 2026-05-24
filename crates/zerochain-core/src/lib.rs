//! Core workflow model: stages, CONTEXT.md parsing, execution DAG.

pub mod context;
pub mod error;
pub(crate) mod frontmatter;
pub mod jj;
pub(crate) mod lua_engine;
pub mod plan;
pub mod stage;
pub mod task;
pub mod template;
pub mod workflow;

pub use context::Context;
pub use error::{Error, Result};
pub use stage::{Stage, StageId};
pub use task::Task;
pub use workflow::Workflow;
pub use lua_engine::{LuaContext, create_sandboxed_vm, load_shared_store, run_hook, save_shared_store};
