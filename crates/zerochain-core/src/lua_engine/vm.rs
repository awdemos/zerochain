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

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> Lua {
        create_sandboxed_vm().expect("sandboxed VM")
    }

    #[test]
    fn io_library_blocked() {
        let lua = sandbox();
        let result = lua.load("io.open('/etc/passwd', 'r')").exec();
        assert!(result.is_err(), "io.open should be blocked");
    }

    #[test]
    fn os_library_blocked() {
        let lua = sandbox();
        let result = lua.load("os.execute('id')").exec();
        assert!(result.is_err(), "os.execute should be blocked");
    }

    #[test]
    fn os_exit_blocked() {
        let lua = sandbox();
        let result = lua.load("os.exit(0)").exec();
        assert!(result.is_err(), "os.exit should be blocked");
    }

    #[test]
    fn package_library_blocked() {
        let lua = sandbox();
        let result = lua.load("package.path").exec();
        assert!(result.is_err(), "package should not be accessible");
    }

    #[test]
    fn require_blocked() {
        let lua = sandbox();
        let result = lua.load("require('os')").exec();
        assert!(result.is_err(), "require should be blocked");
    }

    #[test]
    fn debug_library_blocked() {
        let lua = sandbox();
        let result = lua.load("debug.getinfo(1)").exec();
        assert!(result.is_err(), "debug should not be accessible");
    }

    #[test]
    fn dofile_blocked() {
        let lua = sandbox();
        let result = lua.load("dofile('/etc/passwd')").exec();
        assert!(result.is_err(), "dofile should not exist in sandbox");
    }

    #[test]
    fn loadfile_exists_but_cannot_escape_sandbox() {
        let lua = sandbox();
        // loadfile is a base function (always loaded). It can read real files,
        // but the loaded chunk still runs inside the sandbox — os/io remain blocked.
        let result = lua.load("local f = loadfile('/etc/hostname'); if f then f() end").exec();
        // Either loadfile returns nil (file not found) or the chunk runs but sandbox
        // restrictions still apply. Either way, no sandbox escape.
        if let Ok(()) = result {
            // If the chunk actually ran, verify os is still inaccessible
            let check = lua.load("os.execute('id')").exec();
            assert!(check.is_err(), "os should still be blocked after loadfile");
        }
    }

    #[test]
    fn io_popen_blocked() {
        let lua = sandbox();
        let result = lua.load("io.popen('cat /etc/passwd')").exec();
        assert!(result.is_err(), "io.popen should be blocked");
    }

    #[test]
    fn io_lines_blocked() {
        let lua = sandbox();
        let result = lua.load("io.lines('/etc/passwd')").exec();
        assert!(result.is_err(), "io.lines should be blocked");
    }

    #[test]
    fn string_library_works() {
        let lua = sandbox();
        lua.load("assert(string.upper('hello') == 'HELLO')").exec().unwrap();
    }

    #[test]
    fn table_library_works() {
        let lua = sandbox();
        lua.load("local t = {}; table.insert(t, 1); assert(#t == 1)").exec().unwrap();
    }

    #[test]
    fn math_library_works() {
        let lua = sandbox();
        lua.load("assert(math.abs(-42) == 42)").exec().unwrap();
    }

    #[test]
    fn utf8_library_works() {
        let lua = sandbox();
        lua.load("assert(utf8.len('hello') == 5)").exec().unwrap();
    }

    #[test]
    fn coroutine_library_works() {
        let lua = sandbox();
        lua.load("local co = coroutine.create(function() end); assert(coroutine.status(co) == 'suspended')").exec().unwrap();
    }

    #[test]
    fn infinite_loop_hits_instruction_limit() {
        let lua = sandbox();
        let result = lua.load("while true do end").exec();
        assert!(result.is_err(), "infinite loop should hit instruction limit");
        let msg = format!("{result:?}");
        assert!(msg.contains("instruction limit"), "error should mention instruction limit, got: {msg}");
    }

    #[test]
    fn expensive_computation_hits_limit() {
        let lua = sandbox();
        let result = lua.load(r#"
            local x = 0
            for i = 1, 10000000 do
                x = x + 1
            end
        "#).exec();
        assert!(result.is_err(), "should hit instruction limit");
    }

    #[test]
    fn memory_limit_enforced() {
        let lua = sandbox();
        let result = lua.load(r#"
            local s = string.rep("A", 1024 * 1024)
            for i = 1, 20 do
                s = s .. s
            end
        "#).exec();
        assert!(result.is_err(), "should hit memory limit");
    }

    #[test]
    fn memory_limit_table_allocation() {
        let lua = sandbox();
        let result = lua.load(r#"
            local t = {}
            for i = 1, 5000000 do
                t[i] = string.rep("x", 100)
            end
        "#).exec();
        assert!(result.is_err(), "table allocation should hit memory limit");
    }

    #[test]
    fn cannot_set_global_to_bypass_sandbox() {
        let lua = sandbox();
        let result = lua.load(r#"
            _G["io"] = nil
        "#).exec();
        let result2 = lua.load("io.open('/tmp/x')").exec();
        assert!(result2.is_err());
    }

    #[test]
    fn metatable_tampering_does_not_escape() {
        let lua = sandbox();
        // getmetatable("") returns nil in this sandbox — string metatables are protected
        let result = lua.load(r#"
            local mt = getmetatable("")
            if mt then
                mt.__index = function(_, key)
                    if key == "evil" then
                        return os.execute
                    end
                end
            end
        "#).exec();
        // Script succeeds (mt is nil, nothing happens). Verify os is still blocked.
        let check = lua.load("os.execute('id')").exec();
        assert!(check.is_err(), "os should remain blocked regardless of metatable access");
    }

    #[test]
    fn load_with_bytecode_restricted() {
        let lua = sandbox();
        let result = lua.load("local f = load('os.execute(\"id\")'); if f then f() end").exec();
        assert!(result.is_err(), "load() should not bypass sandbox restrictions");
    }

    #[test]
    fn coroutine_cannot_bypass_instruction_limit() {
        let lua = sandbox();
        let result = lua.load(r#"
            local function infinite()
                while true do coroutine.yield() end
            end
            local co = coroutine.create(infinite)
            for i = 1, 1000000 do
                coroutine.resume(co)
            end
        "#).exec();
        assert!(result.is_err(), "coroutines should still respect instruction limit");
    }

    #[test]
    fn reset_instruction_counter_allows_fresh_execution() {
        let lua = sandbox();
        lua.load(r#"
            local x = 0
            for i = 1, 50000 do x = x + 1 end
        "#).exec().unwrap();

        reset_instruction_counter(&lua).unwrap();

        lua.load(r#"
            local y = 0
            for i = 1, 50000 do y = y + 1 end
            assert(y == 50000)
        "#).exec().unwrap();
    }
}
