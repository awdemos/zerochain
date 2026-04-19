use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mlua::{Lua, UserData, UserDataMethods, Value};

use crate::error::{Error, Result};

fn lua_err(e: mlua::Error) -> Error {
    Error::Lua {
        message: e.to_string(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct HookResults {
    pub skip: bool,
    pub insert_after: Vec<String>,
    pub remove_stages: Vec<String>,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LuaContext {
    pub stage_raw: String,
    pub stage_path: PathBuf,
    pub workflow_root: PathBuf,
    pub output_content: Option<String>,
    pub token_usage: Option<u64>,
    pub env_vars: HashMap<String, String>,
    pub skip: bool,
    pub hooks: HookResults,
    pub shared_store: Arc<Mutex<HashMap<String, serde_json::Value>>>,
}

impl UserData for LuaContext {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("get_env", |_, ctx, key: String| {
            Ok(ctx.env_vars.get(&key).cloned())
        });

        methods.add_method("read_output", |_, ctx, ()| {
            Ok(ctx.output_content.clone())
        });

        methods.add_method("token_usage", |_, ctx, ()| Ok(ctx.token_usage));

        methods.add_method_mut("set_skip", |_, ctx, skip: bool| {
            ctx.skip = skip;
            Ok(())
        });

        methods.add_method("list_stages", |_, ctx, ()| -> mlua::Result<Vec<String>> {
            let mut stages = Vec::new();
            let entries = std::fs::read_dir(&ctx.workflow_root).map_err(|e| {
                mlua::Error::runtime(format!("failed to read workflow dir: {e}"))
            })?;
            for entry in entries {
                let entry = entry.map_err(|e| mlua::Error::runtime(format!("dir entry: {e}")))?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if crate::stage::StageId::parse(&name).is_ok() {
                    stages.push(name);
                }
            }
            stages.sort();
            Ok(stages)
        });

        methods.add_method("stage_complete", |_, ctx, stage: String| {
            let marker = ctx.workflow_root.join(&stage).join(".complete");
            Ok(marker.exists())
        });

        methods.add_method("stage_output", |_, ctx, stage: String| -> mlua::Result<Option<String>> {
            let result_path = ctx
                .workflow_root
                .join(&stage)
                .join("output")
                .join("result.md");
            if result_path.exists() {
                Ok(Some(std::fs::read_to_string(&result_path).map_err(
                    |e| mlua::Error::runtime(format!("failed to read stage output: {e}")),
                )?))
            } else {
                Ok(None)
            }
        });

        methods.add_method_mut("insert_stage_after", |_, ctx, stage_name: String| {
            ctx.hooks.insert_after.push(stage_name);
            Ok(())
        });

        methods.add_method_mut("remove_stage", |_, ctx, stage_name: String| {
            ctx.hooks.remove_stages.push(stage_name);
            Ok(())
        });

        methods.add_method("store", |_, ctx, (key, value): (String, mlua::Value)| {
            let json_val = lua_value_to_json(&value);
            let mut store = ctx.shared_store.lock().map_err(|_| {
                mlua::Error::runtime("shared store lock poisoned")
            })?;
            store.insert(key, json_val);
            Ok(())
        });

        methods.add_method("load", |lua, ctx, key: String| {
            let store = ctx.shared_store.lock().map_err(|_| {
                mlua::Error::runtime("shared store lock poisoned")
            })?;
            match store.get(&key) {
                Some(v) => json_to_lua_value(lua, v),
                None => Ok(Value::Nil),
            }
        });
    }
}

fn lua_value_to_json(val: &mlua::Value) -> serde_json::Value {
    match val {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Integer(n) => serde_json::json!(*n),
        Value::Number(n) => serde_json::json!(*n),
        Value::String(s) => {
            let str_val = s.to_str().map(|v| v.to_string()).unwrap_or_default();
            serde_json::Value::String(str_val)
        }
        Value::Table(t) => {
            let mut map = serde_json::Map::new();
            for (k, v) in t.pairs::<mlua::Value, mlua::Value>().flatten() {
                let key_str = match &k {
                    Value::String(s) => s.to_str().map(|v| v.to_string()).unwrap_or_default(),
                    Value::Integer(n) => n.to_string(),
                    _ => continue,
                };
                map.insert(key_str, lua_value_to_json(&v));
            }
            serde_json::Value::Object(map)
        }
        _ => serde_json::Value::Null,
    }
}

fn json_to_lua_value(lua: &Lua, val: &serde_json::Value) -> mlua::Result<mlua::Value> {
    match val {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Integer(i))
            } else {
                Ok(Value::Number(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(lua.create_string(s)?)),
        _ => Ok(Value::Nil),
    }
}

impl LuaContext {
    pub fn new(stage_raw: &str, stage_path: &Path, workflow_root: &Path) -> Self {
        let mut env_vars = HashMap::new();
        for key in &[
            "ZEROCHAIN_PROVIDER_PROFILE",
            "ZEROCHAIN_CAPTURE_REASONING",
            "ZEROCHAIN_THINKING_MODE",
            "ZEROCHAIN_MODEL",
            "ZEROCHAIN_BASE_URL",
        ] {
            if let Ok(val) = std::env::var(key) {
                env_vars.insert(key.to_string(), val);
            }
        }
        Self {
            stage_raw: stage_raw.to_string(),
            stage_path: stage_path.to_path_buf(),
            workflow_root: workflow_root.to_path_buf(),
            output_content: None,
            token_usage: None,
            env_vars,
            skip: false,
            hooks: HookResults::default(),
            shared_store: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_output(mut self, content: &str, tokens: u64) -> Self {
        self.output_content = Some(content.to_string());
        self.token_usage = Some(tokens);
        self
    }

    pub fn with_shared_store(
        mut self,
        store: Arc<Mutex<HashMap<String, serde_json::Value>>>,
    ) -> Self {
        self.shared_store = store;
        self
    }
}

pub fn run_hook(
    lua: &Lua,
    hook_name: &str,
    ctx: &mut LuaContext,
    script: &str,
) -> Result<()> {
    super::vm::reset_instruction_counter(lua)?;

    lua.globals()
        .set("ctx", ctx.clone())
        .map_err(lua_err)?;

    let hook_call = format!(
        "{script}\nlocal __zc_hook = {hook_name}\nif type(__zc_hook) == 'function' then return __zc_hook(ctx) end"
    );

    let chunk = lua.load(&hook_call).set_name("CONTEXT.lua");
    chunk.exec().map_err(lua_err)?;

    Ok(())
}

pub fn load_shared_store(
    workflow_root: &Path,
) -> Arc<Mutex<HashMap<String, serde_json::Value>>> {
    let store_path = workflow_root.join(".state").join("lua_store.json");
    let map: HashMap<String, serde_json::Value> = if store_path.exists() {
        std::fs::read_to_string(&store_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        HashMap::new()
    };
    Arc::new(Mutex::new(map))
}

pub fn save_shared_store(
    workflow_root: &Path,
    store: &Arc<Mutex<HashMap<String, serde_json::Value>>>,
) -> Result<()> {
    let state_dir = workflow_root.join(".state");
    std::fs::create_dir_all(&state_dir).map_err(|e| Error::Io {
        path: state_dir.clone(),
        source: e,
    })?;
    let store_path = state_dir.join("lua_store.json");
    let map = store.lock().map_err(|_| Error::Lua {
        message: "shared store lock poisoned".into(),
    })?;
    let json = serde_json::to_string_pretty(&*map).map_err(|e| Error::Lua {
        message: format!("failed to serialize store: {e}"),
    })?;
    std::fs::write(&store_path, json).map_err(|e| Error::Io {
        path: store_path,
        source: e,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_context_new_populates_env() {
        std::env::set_var("ZEROCHAIN_MODEL", "test-model");
        let ctx = LuaContext::new("01_test", Path::new("/tmp"), Path::new("/tmp/wf"));
        assert_eq!(
            ctx.env_vars.get("ZEROCHAIN_MODEL"),
            Some(&"test-model".to_string())
        );
        assert!(!ctx.skip);
        std::env::remove_var("ZEROCHAIN_MODEL");
    }

    #[test]
    fn lua_context_with_output() {
        let ctx =
            LuaContext::new("01_test", Path::new("/tmp"), Path::new("/tmp/wf"))
                .with_output("result text", 500);
        assert_eq!(ctx.output_content.as_deref(), Some("result text"));
        assert_eq!(ctx.token_usage, Some(500));
    }

    #[test]
    fn shared_store_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = load_shared_store(tmp.path());
        {
            let mut s = store.lock().unwrap();
            s.insert("key".into(), serde_json::json!("value"));
        }
        save_shared_store(tmp.path(), &store).unwrap();

        let store2 = load_shared_store(tmp.path());
        let s2 = store2.lock().unwrap();
        assert_eq!(s2.get("key").unwrap().as_str(), Some("value"));
    }
}
