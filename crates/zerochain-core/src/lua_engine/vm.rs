use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Value, VmState};

use crate::error::Result;

const MEMORY_LIMIT_BYTES: usize = 10 * 1024 * 1024;
const INSTRUCTION_LIMIT: i64 = 1_000_000;
const INSTRUCTION_HOOK_EVERY: i64 = 100_000;
const VM_POOL_MAX_SIZE: usize = 16;
const GLOBAL_SNAPSHOT_REGISTRY_KEY: &str = "__zc_global_snapshot";

static VM_POOL: OnceLock<Mutex<Vec<Lua>>> = OnceLock::new();

fn vm_pool() -> &'static Mutex<Vec<Lua>> {
    VM_POOL.get_or_init(|| Mutex::new(Vec::with_capacity(VM_POOL_MAX_SIZE)))
}

/// RAII guard around a pooled Lua VM. Returns the VM to the pool on drop.
pub struct PooledLua {
    lua: Option<Lua>,
}

impl PooledLua {
    pub fn get(&self) -> &Lua {
        self.lua.as_ref().expect("PooledLua already consumed")
    }

    pub fn into_inner(mut self) -> Lua {
        self.lua.take().expect("PooledLua already consumed")
    }
}

impl Drop for PooledLua {
    fn drop(&mut self) {
        if let Some(lua) = self.lua.take() {
            if reset_vm_state(&lua).is_ok() {
                if let Ok(mut pool) = vm_pool().lock() {
                    if pool.len() < VM_POOL_MAX_SIZE {
                        pool.push(lua);
                    }
                }
            }
        }
    }
}

/// Install (or reinstall) the instruction-budget hook with a fresh counter.
///
/// The counter lives in Rust state captured by the hook closure — sandboxed
/// scripts can read and write a `__zc_hook_count` global all they want, but it
/// no longer has any effect on the budget. The counter is an `i64` advanced
/// with a saturating compare-and-swap, so scripts cannot provoke an overflow.
fn install_instruction_hook(lua: &Lua) -> Result<()> {
    let count = AtomicI64::new(0);
    let triggers = HookTriggers::new().every_nth_instruction(INSTRUCTION_HOOK_EVERY as u32);
    lua.set_hook(triggers, move |_lua, _debug| {
        let mut current = count.load(Ordering::Relaxed);
        loop {
            let new_count = current.saturating_add(INSTRUCTION_HOOK_EVERY);
            if new_count > INSTRUCTION_LIMIT {
                return Err(mlua::Error::runtime(format!(
                    "Lua script exceeded instruction limit ({INSTRUCTION_LIMIT})"
                )));
            }
            match count.compare_exchange_weak(
                current,
                new_count,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(VmState::Continue),
                Err(actual) => current = actual,
            }
        }
    })
    .map_err(|e| crate::error::Error::Lua {
        message: format!("failed to set hook: {e}"),
    })
}

/// Record the set of global keys present at VM creation so `reset_vm_state`
/// can later delete anything a stage script added. Stored in the Lua registry,
/// which sandboxed scripts cannot reach.
fn snapshot_globals(lua: &Lua) -> Result<()> {
    let snapshot = lua.create_table().map_err(|e| crate::error::Error::Lua {
        message: format!("failed to create global snapshot: {e}"),
    })?;
    for pair in lua.globals().pairs::<Value, Value>() {
        let Ok((key, _)) = pair else { continue };
        let key_str = match key {
            Value::String(s) => s.to_str().map(|k| k.to_string()).unwrap_or_default(),
            Value::Integer(i) => i.to_string(),
            _ => continue,
        };
        let _ = snapshot.set(key_str, true);
    }
    lua.set_named_registry_value(GLOBAL_SNAPSHOT_REGISTRY_KEY, snapshot)
        .map_err(|e| crate::error::Error::Lua {
            message: format!("failed to store global snapshot: {e}"),
        })
}

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

    install_instruction_hook(&lua)?;
    snapshot_globals(&lua)?;

    Ok(lua)
}

pub fn reset_instruction_counter(lua: &Lua) -> Result<()> {
    // Reinstalling the hook replaces the closure (and its counter) with a
    // fresh one starting at zero.
    install_instruction_hook(lua)
}

fn reset_vm_state(lua: &Lua) -> Result<()> {
    reset_instruction_counter(lua)?;

    // Delete every global a stage script may have defined (hook functions,
    // markers, tampered state) so nothing leaks into later stages that reuse
    // this pooled VM. Globals present at creation are left untouched; `ctx`
    // is not in the snapshot, so it is cleared here as well.
    let snapshot: Option<mlua::Table> = lua.named_registry_value(GLOBAL_SNAPSHOT_REGISTRY_KEY).ok();
    let Some(snapshot) = snapshot else {
        let _ = lua.globals().set("ctx", Value::Nil);
        return Ok(());
    };

    let globals = lua.globals();
    let mut to_remove: Vec<Value> = Vec::new();
    for pair in globals.pairs::<Value, Value>() {
        let Ok((key, _)) = pair else { continue };
        let in_snapshot = match &key {
            Value::String(s) => s
                .to_str()
                .map(|k| snapshot.get::<bool>(k).unwrap_or(false))
                .unwrap_or(false),
            Value::Integer(i) => snapshot.get::<bool>(i.to_string()).unwrap_or(false),
            _ => false,
        };
        if !in_snapshot {
            to_remove.push(key);
        }
    }
    for key in to_remove {
        let _ = globals.set(key, Value::Nil);
    }
    Ok(())
}

pub fn acquire_sandboxed_vm() -> Result<PooledLua> {
    if let Ok(mut pool) = vm_pool().lock() {
        if let Some(lua) = pool.pop() {
            reset_vm_state(&lua)?;
            return Ok(PooledLua { lua: Some(lua) });
        }
    }
    Ok(PooledLua {
        lua: Some(create_sandboxed_vm()?),
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
        let result = lua
            .load("local f = loadfile('/etc/hostname'); if f then f() end")
            .exec();
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
        lua.load("assert(string.upper('hello') == 'HELLO')")
            .exec()
            .unwrap();
    }

    #[test]
    fn table_library_works() {
        let lua = sandbox();
        lua.load("local t = {}; table.insert(t, 1); assert(#t == 1)")
            .exec()
            .unwrap();
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
        assert!(
            result.is_err(),
            "infinite loop should hit instruction limit"
        );
        let msg = format!("{result:?}");
        assert!(
            msg.contains("instruction limit"),
            "error should mention instruction limit, got: {msg}"
        );
    }

    #[test]
    fn expensive_computation_hits_limit() {
        let lua = sandbox();
        let result = lua
            .load(
                r"
            local x = 0
            for i = 1, 10000000 do
                x = x + 1
            end
        ",
            )
            .exec();
        assert!(result.is_err(), "should hit instruction limit");
    }

    #[test]
    fn memory_limit_enforced() {
        let lua = sandbox();
        let result = lua
            .load(
                r#"
            local s = string.rep("A", 1024 * 1024)
            for i = 1, 20 do
                s = s .. s
            end
        "#,
            )
            .exec();
        assert!(result.is_err(), "should hit memory limit");
    }

    #[test]
    fn memory_limit_table_allocation() {
        let lua = sandbox();
        let result = lua
            .load(
                r#"
            local t = {}
            for i = 1, 5000000 do
                t[i] = string.rep("x", 100)
            end
        "#,
            )
            .exec();
        assert!(result.is_err(), "table allocation should hit memory limit");
    }

    #[test]
    fn cannot_set_global_to_bypass_sandbox() {
        let lua = sandbox();
        let _result = lua
            .load(
                r#"
            _G["io"] = nil
        "#,
            )
            .exec();
        let result2 = lua.load("io.open('/tmp/x')").exec();
        assert!(result2.is_err());
    }

    #[test]
    fn metatable_tampering_does_not_escape() {
        let lua = sandbox();
        // getmetatable("") returns nil in this sandbox — string metatables are protected
        let _result = lua
            .load(
                r#"
            local mt = getmetatable("")
            if mt then
                mt.__index = function(_, key)
                    if key == "evil" then
                        return os.execute
                    end
                end
            end
        "#,
            )
            .exec();
        // Script succeeds (mt is nil, nothing happens). Verify os is still blocked.
        let check = lua.load("os.execute('id')").exec();
        assert!(
            check.is_err(),
            "os should remain blocked regardless of metatable access"
        );
    }

    #[test]
    fn load_with_bytecode_restricted() {
        let lua = sandbox();
        let result = lua
            .load("local f = load('os.execute(\"id\")'); if f then f() end")
            .exec();
        assert!(
            result.is_err(),
            "load() should not bypass sandbox restrictions"
        );
    }

    #[test]
    fn coroutine_cannot_bypass_instruction_limit() {
        let lua = sandbox();
        let result = lua
            .load(
                r"
            local function infinite()
                while true do coroutine.yield() end
            end
            local co = coroutine.create(infinite)
            for i = 1, 1000000 do
                coroutine.resume(co)
            end
        ",
            )
            .exec();
        assert!(
            result.is_err(),
            "coroutines should still respect instruction limit"
        );
    }

    #[test]
    fn reset_instruction_counter_allows_fresh_execution() {
        let lua = sandbox();
        lua.load(
            r"
            local x = 0
            for i = 1, 50000 do x = x + 1 end
        ",
        )
        .exec()
        .unwrap();

        reset_instruction_counter(&lua).unwrap();

        lua.load(
            r"
            local y = 0
            for i = 1, 50000 do y = y + 1 end
            assert(y == 50000)
        ",
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn tampering_with_hook_count_global_cannot_multiply_budget() {
        let lua = sandbox();
        // The budget counter lives in Rust state now; presetting the legacy
        // Lua global must have no effect and the loop must still hit the limit.
        let result = lua
            .load("__zc_hook_count = -3000000\nwhile true do end")
            .exec();
        assert!(
            result.is_err(),
            "infinite loop should hit instruction limit despite global tampering"
        );
        let msg = format!("{result:?}");
        assert!(
            msg.contains("instruction limit"),
            "error should mention instruction limit, got: {msg}"
        );
    }

    #[test]
    fn huge_hook_count_global_does_not_overflow_or_panic() {
        let lua = sandbox();
        // i32::MAX + 100_000 would overflow the old i32 counter (debug panic /
        // release wrap). The Rust-side i64 counter is unaffected.
        let result = lua
            .load("__zc_hook_count = 2147483647\nlocal x = 0\nfor i = 1, 100000 do x = x + 1 end")
            .exec();
        assert!(
            result.is_ok(),
            "small loop after tampering must run without panic, got: {result:?}"
        );
    }

    #[test]
    fn reset_vm_state_removes_script_defined_globals() {
        let lua = sandbox();
        lua.load("on_validate = function(ctx) return true end\nmarker = 'x'")
            .exec()
            .unwrap();
        assert!(lua
            .globals()
            .get::<Option<mlua::Function>>("on_validate")
            .unwrap()
            .is_some());
        assert!(lua
            .globals()
            .get::<Option<mlua::String>>("marker")
            .unwrap()
            .is_some());

        reset_vm_state(&lua).unwrap();

        assert!(
            lua.globals()
                .get::<Option<mlua::Function>>("on_validate")
                .unwrap()
                .is_none(),
            "hook functions must not survive a reset"
        );
        assert!(
            lua.globals()
                .get::<Option<mlua::String>>("marker")
                .unwrap()
                .is_none(),
            "script globals must not survive a reset"
        );
        // Globals present at VM creation are untouched.
        assert!(lua
            .globals()
            .get::<Option<mlua::Table>>("string")
            .unwrap()
            .is_some());
        assert!(lua
            .globals()
            .get::<Option<mlua::Table>>("_G")
            .unwrap()
            .is_some());
    }
}
