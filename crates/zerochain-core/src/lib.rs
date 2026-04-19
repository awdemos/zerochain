//! Core workflow model: stages, CONTEXT.md parsing, execution DAG.

pub mod context;
pub mod error;
pub mod jj;
pub mod lua_engine;
pub mod plan;
pub mod stage;
pub mod task;
pub mod template;
pub mod workflow;

pub use context::{Context, ContextFrontmatter, MultimodalInput};
pub use error::{Error, Result};
pub use jj::{CommitEntry, JjManager};
pub use lua_engine::{
    HookResults, LuaContext, eval_context_lua, eval_context_lua_file, execute_hook,
    load_shared_store, save_shared_store, create_sandboxed_vm,
};
pub use plan::{ExecutionPlan, StageGroup, StageNode, StageState};
pub use stage::{Stage, StageId};
pub use task::{Task, TaskExecution};
pub use template::{StageDef, Template, TemplateRegistry};
pub use workflow::Workflow;
