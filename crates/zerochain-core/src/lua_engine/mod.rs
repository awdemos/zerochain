pub mod api;
pub mod config;
pub mod vm;

use crate::error::Result;
use crate::frontmatter::ContextFrontmatter;

pub use api::{load_shared_store, run_hook, save_shared_store, LuaContext};
pub use config::eval_config_script;
pub use vm::create_sandboxed_vm;

pub fn eval_context_lua(script: &str) -> Result<ContextFrontmatter> {
    let lua = create_sandboxed_vm()?;
    eval_config_script(&lua, script)
}
