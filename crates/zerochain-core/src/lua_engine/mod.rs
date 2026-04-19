pub mod api;
pub mod config;
pub mod vm;

use std::path::Path;

use crate::context::ContextFrontmatter;
use crate::error::Result;

pub use api::{load_shared_store, run_hook, save_shared_store, HookResults, LuaContext};
pub use config::eval_config_script;
pub use vm::create_sandboxed_vm;

pub fn eval_context_lua(script: &str) -> Result<ContextFrontmatter> {
    let lua = create_sandboxed_vm()?;
    eval_config_script(&lua, script)
}

pub fn eval_context_lua_file(path: &Path) -> Result<ContextFrontmatter> {
    let script = std::fs::read_to_string(path).map_err(|e| crate::error::Error::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    eval_context_lua(&script)
}


