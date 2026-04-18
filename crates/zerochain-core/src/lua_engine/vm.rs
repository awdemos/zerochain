use mlua::{HookTriggers, Lua, LuaOptions, StdLib, VmState};

use crate::error::Result;

const MEMORY_LIMIT_BYTES: usize = 10 * 1024 * 1024;
const INSTRUCTION_LIMIT: i32 = 1_000_000;

pub fn create_sandboxed_vm() -> Result<Lua> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE,
        LuaOptions::default(),
    )
    .map_err(|e| crate::error::Error::Lua {
        message: format!("failed to create Lua VM: {e}"),
    })?;

    lua.set_memory_limit(MEMORY_LIMIT_BYTES)
        .map_err(|e| crate::error::Error::Lua {
            message: format!("failed to set memory limit: {e}"),
        })?;

    let hook_limit = INSTRUCTION_LIMIT;
    let triggers = HookTriggers::new().every_nth_instruction(100_000);
    lua.set_hook(triggers, move |lua, _debug| {
        let count: i32 = lua
            .globals()
            .get::<Option<i32>>("__zc_hook_count")
            .unwrap_or(None)
            .unwrap_or(0);
        let new_count = count + 100_000;
        if new_count > hook_limit {
            return Err(mlua::Error::runtime(format!(
                "Lua script exceeded instruction limit ({hook_limit})"
            )));
        }
        lua.globals().set("__zc_hook_count", new_count)?;
        Ok(VmState::Continue)
    })
    .map_err(|e| crate::error::Error::Lua {
        message: format!("failed to set hook: {e}"),
    })?;

    Ok(lua)
}

pub fn reset_instruction_counter(lua: &Lua) -> Result<()> {
    lua.globals()
        .set("__zc_hook_count", 0i32)
        .map_err(|e| crate::error::Error::Lua {
            message: format!("failed to reset instruction counter: {e}"),
        })
}
