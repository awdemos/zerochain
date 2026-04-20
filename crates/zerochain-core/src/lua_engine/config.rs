use mlua::{Lua, Table, Value};

use crate::frontmatter::{ContextFrontmatter, MultimodalInput};
use crate::error::{Error, Result};

fn lua_err(e: &mlua::Error) -> Error {
    Error::Lua {
        message: e.to_string(),
    }
}

fn get_string(table: &Table, key: &str) -> Result<Option<String>> {
    match table.get::<Value>(key).map_err(|e| lua_err(&e))? {
        Value::String(s) => Ok(Some(s.to_str().map_err(|e| lua_err(&e))?.to_string())),
        _ => Ok(None),
    }
}

fn get_bool(table: &Table, key: &str) -> Result<bool> {
    match table.get::<Value>(key).map_err(|e| lua_err(&e))? {
        Value::Boolean(b) => Ok(b),
        _ => Ok(false),
    }
}

fn get_u64(table: &Table, key: &str) -> Result<Option<u64>> {
    match table.get::<Value>(key).map_err(|e| lua_err(&e))? {
        Value::Integer(n) => Ok(Some(n.try_into().unwrap_or(0))),
        _ => Ok(None),
    }
}

fn parse_multimodal(table: &Table, key: &str) -> Result<Vec<MultimodalInput>> {
    match table.get::<Value>(key).map_err(|e| lua_err(&e))? {
        Value::Table(arr) => {
            let mut result = Vec::new();
            for pair in arr.sequence_values::<Table>() {
                let t = pair.map_err(|e| Error::Lua {
                    message: format!("multimodal_input parse error: {e}"),
                })?;
                let input_type = t
                    .get::<Option<String>>("type")
                    .map_err(|e| lua_err(&e))?
                    .unwrap_or_else(|| "image".to_string());
                let path = t
                    .get::<Option<String>>("path")
                    .map_err(|e| lua_err(&e))?
                    .unwrap_or_default();
                let detail = t.get::<Option<String>>("detail").map_err(|e| lua_err(&e))?;
                result.push(MultimodalInput {
                    input_type,
                    path,
                    detail,
                });
            }
            Ok(result)
        }
        _ => Ok(vec![]),
    }
}

pub fn table_to_frontmatter(table: &Table) -> Result<ContextFrontmatter> {
    Ok(ContextFrontmatter {
        role: get_string(table, "role")?,
        container: get_string(table, "container")?,
        command: get_string(table, "command")?,
        human_gate: get_bool(table, "human_gate")?,
        timeout: get_u64(table, "timeout")?,
        network: get_string(table, "network")?,
        definition_of_done: get_string(table, "definition_of_done")?,
        provider_profile: get_string(table, "provider_profile")?,
        thinking_mode: get_string(table, "thinking_mode")?,
        capture_reasoning: get_bool(table, "capture_reasoning")?,
        multimodal_input: parse_multimodal(table, "multimodal_input")?,
    })
}

pub fn eval_config_script(lua: &Lua, script: &str) -> Result<ContextFrontmatter> {
    super::vm::reset_instruction_counter(lua)?;

    let chunk = lua.load(script).set_name("CONTEXT.lua");
    let value: Value = chunk.eval().map_err(|e| lua_err(&e))?;

    match value {
        Value::Table(table) => table_to_frontmatter(&table),
        _ => Err(Error::Lua {
            message: "CONTEXT.lua must return a table".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_lua() -> Lua {
        super::super::vm::create_sandboxed_vm().unwrap()
    }

    #[test]
    fn minimal_config_returns_defaults() {
        let lua = setup_lua();
        let fm = eval_config_script(&lua, r#"return { role = "helper" }"#).unwrap();
        assert_eq!(fm.role.as_deref(), Some("helper"));
        assert!(!fm.human_gate);
        assert!(fm.provider_profile.is_none());
        assert!(fm.thinking_mode.is_none());
        assert!(!fm.capture_reasoning);
        assert!(fm.multimodal_input.is_empty());
    }

    #[test]
    fn full_config_all_fields() {
        let lua = setup_lua();
        let fm = eval_config_script(
            &lua,
            r#"return {
                role = "reviewer",
                container = "rust:1.80",
                command = "cargo test",
                human_gate = true,
                timeout = 300,
                network = "host",
                definition_of_done = "all tests pass",
                provider_profile = "kimi-k2",
                thinking_mode = "extended",
                capture_reasoning = true,
            }"#,
        )
        .unwrap();
        assert_eq!(fm.role.as_deref(), Some("reviewer"));
        assert_eq!(fm.container.as_deref(), Some("rust:1.80"));
        assert_eq!(fm.command.as_deref(), Some("cargo test"));
        assert!(fm.human_gate);
        assert_eq!(fm.timeout, Some(300));
        assert_eq!(fm.network.as_deref(), Some("host"));
        assert_eq!(fm.definition_of_done.as_deref(), Some("all tests pass"));
        assert_eq!(fm.provider_profile.as_deref(), Some("kimi-k2"));
        assert_eq!(fm.thinking_mode.as_deref(), Some("extended"));
        assert!(fm.capture_reasoning);
    }

    #[test]
    fn config_with_multimodal() {
        let lua = setup_lua();
        let fm = eval_config_script(
            &lua,
            r#"return {
                multimodal_input = {
                    { type = "image", path = "./wire.png", detail = "high" },
                },
            }"#,
        )
        .unwrap();
        assert_eq!(fm.multimodal_input.len(), 1);
        assert_eq!(fm.multimodal_input[0].input_type, "image");
        assert_eq!(fm.multimodal_input[0].path, "./wire.png");
        assert_eq!(fm.multimodal_input[0].detail.as_deref(), Some("high"));
    }

    #[test]
    fn empty_table_returns_defaults() {
        let lua = setup_lua();
        let fm = eval_config_script(&lua, "return {}").unwrap();
        assert!(fm.role.is_none());
        assert!(!fm.human_gate);
    }

    #[test]
    fn non_table_return_errors() {
        let lua = setup_lua();
        let result = eval_config_script(&lua, "return 'hello'");
        assert!(result.is_err());
    }

    #[test]
    fn invalid_lua_syntax_errors() {
        let lua = setup_lua();
        let result = eval_config_script(&lua, "return {");
        assert!(result.is_err());
    }

    #[test]
    fn io_library_is_blocked() {
        let lua = setup_lua();
        let result = eval_config_script(&lua, "io.open('/etc/passwd', 'r')");
        assert!(result.is_err());
    }

    #[test]
    fn os_library_is_blocked() {
        let lua = setup_lua();
        let result = eval_config_script(&lua, "os.execute('ls')");
        assert!(result.is_err());
    }
}
