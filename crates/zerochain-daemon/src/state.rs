use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::DaemonError;
use zerochain_core::context::Context as StageContext;
use zerochain_core::lua_engine::{
    create_sandboxed_vm, run_hook, load_shared_store, save_shared_store, LuaContext,
};
use zerochain_core::stage::{Stage, StageId};
use zerochain_core::task::Task;
use zerochain_core::workflow::Workflow;
use zerochain_llm::{
    Content, ImageUrlContent, LLM, LLMConfig, LLMFactory, Message, ProviderId, Role,
    StageContext as LlmStageContext, ThinkingMode, resolve_profile,
};


pub struct InitWorkflowParams<'a> {
    pub name: &'a str,
    pub path: Option<&'a Path>,
    pub template: Option<&'a str>,
}


pub struct AppState {
    pub workspace_root: PathBuf,
    pub workflows: HashMap<String, Workflow>,
}

fn workflow_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".zerochain").join("workflows")
}

fn parse_thinking_mode(val: &str) -> ThinkingMode {
    match val {
        "disabled" => ThinkingMode::Disabled,
        s if s.starts_with("extended") => {
            let budget = s
                .split(':')
                .nth(1)
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(8192);
            ThinkingMode::Extended { budget_tokens: budget }
        }
        _ => ThinkingMode::Default,
    }
}

fn resolve_thinking_mode(ctx: &StageContext) -> ThinkingMode {
    if let Some(ref mode_str) = ctx.frontmatter.thinking_mode {
        return parse_thinking_mode(mode_str);
    }
    if let Ok(env_mode) = std::env::var("ZEROCHAIN_THINKING_MODE") {
        if !env_mode.is_empty() {
            return parse_thinking_mode(&env_mode);
        }
    }
    ThinkingMode::Default
}

fn resolve_profile_name(ctx: &StageContext) -> String {
    if let Some(ref name) = ctx.frontmatter.provider_profile {
        return name.clone();
    }
    std::env::var("ZEROCHAIN_PROVIDER_PROFILE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "generic".to_string())
}

fn resolve_capture_reasoning(ctx: &StageContext) -> bool {
    if ctx.frontmatter.capture_reasoning {
        return true;
    }
    std::env::var("ZEROCHAIN_CAPTURE_REASONING")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s == "true" || s == "1")
        .unwrap_or(false)
}

impl AppState {
    pub fn new(workspace_root: &Path) -> AppState {
        AppState {
            workspace_root: workspace_root.to_path_buf(),
            workflows: HashMap::new(),
        }
    }

    pub async fn load_workflows(&mut self) -> Result<(), DaemonError> {
        let dir = workflow_dir(&self.workspace_root);
        if !dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&dir).await
            .map_err(|e| DaemonError::io(&dir, e))?;
        while let Some(entry) = entries.next_entry().await
            .map_err(|e| DaemonError::io(&dir, e))? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            match Workflow::from_dir(&path).await {
                Ok(wf) => {
                    self.workflows.insert(wf.id.clone(), wf);
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to load workflow");
                }
            }
        }
        Ok(())
    }

    pub fn get_workflow(&self, id: &str) -> Option<&Workflow> {
        self.workflows.get(id)
    }

    pub fn get_workflow_mut(&mut self, id: &str) -> Option<&mut Workflow> {
        self.workflows.get_mut(id)
    }

    pub async fn reload_workflow(&mut self, id: &str) -> Result<(), DaemonError> {
        let wf = self.workflows.get(id).ok_or_else(|| DaemonError::WorkflowNotFound(id.into()))?;
        let root = wf.root.clone();
        let reloaded = Workflow::from_dir(&root).await?;
        self.workflows.insert(id.to_string(), reloaded);
        Ok(())
    }

    pub async fn init_workflow(
        &mut self,
        params: InitWorkflowParams<'_>,
    ) -> Result<Workflow, DaemonError> {
        let InitWorkflowParams { name, path, template } = params;
        let base = path.unwrap_or(&self.workspace_root);
        let wf_base = workflow_dir(base);
        tokio::fs::create_dir_all(&wf_base)
            .await
            .map_err(|e| DaemonError::io(&wf_base, e))?;

        let mut registry = zerochain_core::template::TemplateRegistry::new();
        let builtin_template_dir = self.workspace_root.join("templates");
        if builtin_template_dir.is_dir() {
            if let Err(e) = registry.load_from_dir(&builtin_template_dir) {
                tracing::debug!(error = %e, "no builtin templates directory");
            }
        }

        let named_template = template.and_then(|t| registry.get(t));

        let (stage_names, stage_defs): (Vec<String>, Option<&Vec<zerochain_core::template::StageDef>>) = if let Some(tpl) = named_template {
            (tpl.stage_names(), Some(&tpl.stages))
        } else {
            let names: Vec<String> = template
                .map(|t| {
                    t.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_else(|| vec!["00_spec".into(), "01_implement".into(), "02_verify".into()]);
            (names, None)
        };

        let task = Task::builder(name, name)
            .execution(zerochain_core::task::TaskExecution::new(
                stage_names,
                Some("sequential".into()),
            ))
            .build();

        let workflow = Workflow::init(&task, &wf_base).await?;

        if let Some(defs) = stage_defs {
            for def in defs {
                let stage_dir = workflow.root.join(&def.name);
                let ctx_path = stage_dir.join("CONTEXT.md");

                if let Some(ref src_dir) = def.source_dir {
                    copy_tree_stage(src_dir, &stage_dir).await?;
                } else {
                    tokio::fs::write(&ctx_path, def.to_context_md())
                        .await
                        .map_err(|e| DaemonError::io(&ctx_path, e))?;
                }
            }
        }

        self.workflows.insert(workflow.id.clone(), workflow.clone());
        Ok(workflow)
    }

    pub async fn mark_stage_complete(
        &mut self,
        workflow_id: &str,
        stage_id: &str,
    ) -> Result<(), DaemonError> {
        let wf = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;
        let sid = StageId::parse(stage_id).map_err(|e| DaemonError::InvalidStageId(e.to_string()))?;
        let stage = wf.stage_by_id(&sid).ok_or_else(|| DaemonError::StageNotFound(stage_id.into()))?;

        let marker = stage.path.join(".complete");
        tokio::fs::write(&marker, "").await
            .map_err(|e| DaemonError::io(&marker, e))?;
        let err_marker = stage.path.join(".error");
        if err_marker.exists() {
            tokio::fs::remove_file(&err_marker).await
                .map_err(|e| DaemonError::io(&err_marker, e))?;
        }
        Ok(())
    }

    pub async fn mark_stage_error(
        &mut self,
        workflow_id: &str,
        stage_id: &str,
        feedback: Option<&str>,
    ) -> Result<(), DaemonError> {
        let wf = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;
        let sid = StageId::parse(stage_id).map_err(|e| DaemonError::InvalidStageId(e.to_string()))?;
        let stage = wf.stage_by_id(&sid).ok_or_else(|| DaemonError::StageNotFound(stage_id.into()))?;

        let marker = stage.path.join(".error");
        tokio::fs::write(&marker, feedback.unwrap_or(""))
            .await
            .map_err(|e| DaemonError::io(&marker, e))?;
        Ok(())
    }

    pub fn list_workflows(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for wf in self.workflows.values() {
            let plan = wf.execution_plan();
            let status = if plan.is_complete() {
                "complete"
            } else {
                "active"
            };
            out.push((wf.id.clone(), status.to_string()));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub async fn execute_stage(
        &mut self,
        workflow_id: &str,
        stage: &Stage,
    ) -> Result<(), DaemonError> {
        let llm = self.create_llm()?;
        self.execute_stage_with_llm(workflow_id, stage, llm.as_ref()).await
    }

    pub async fn execute_stage_with_llm(
        &mut self,
        workflow_id: &str,
        stage: &Stage,
        llm: &dyn LLM,
    ) -> Result<(), DaemonError> {
        let ctx = if stage.context_path.exists() {
            Some(StageContext::from_file(&stage.context_path).await?)
        } else {
            None
        };

        let lua_script = {
            let lua_path = stage.path.join("CONTEXT.lua");
            if lua_path.exists() {
                Some(tokio::fs::read_to_string(&lua_path).await
                    .map_err(|e| DaemonError::io(&lua_path, e))?)
            } else {
                None
            }
        };

        let input_content = self.read_input_files(&stage.input_path).await?;

        let profile_name = ctx
            .as_ref()
            .map(resolve_profile_name)
            .unwrap_or_else(|| "generic".to_string());

        let thinking_mode = ctx
            .as_ref()
            .map(resolve_thinking_mode)
            .unwrap_or_default();

        let capture_reasoning = ctx
            .as_ref()
            .map(resolve_capture_reasoning)
            .unwrap_or(false);

        let profile = resolve_profile(&profile_name);

        let model = std::env::var("ZEROCHAIN_MODEL").unwrap_or_else(|_| "gpt-4o".into());
        let mut config = LLMConfig::new(ProviderId::OpenAI, &model);

        if profile_name == "kimi-k2"
            || matches!(&thinking_mode, ThinkingMode::Disabled | ThinkingMode::Extended { .. })
        {
            config = config.with_temperature(1.0);
        }

        let stage_ctx = LlmStageContext {
            thinking_mode,
            capture_reasoning,
        };

        profile.validate_config(&config, &stage_ctx).map_err(|e| {
            DaemonError::ProfileValidation(e.to_string())
        })?;

        let shared_store = match self.workflows.get(workflow_id) {
            Some(wf) => load_shared_store(&wf.root),
            None => load_shared_store(Path::new(".")),
        };

        if let Some(ref script) = lua_script {
            let lua = create_sandboxed_vm()
                .map_err(|e| DaemonError::Lua(format!("Lua VM init failed: {e}")))?;
            let mut lua_ctx = LuaContext::new(
                &stage.id.raw,
                &stage.path,
                &self.workflows.get(workflow_id)
                    .map(|wf| wf.root.clone())
                    .unwrap_or_else(|| stage.path.clone()),
            ).with_shared_store(shared_store.clone());
            run_hook(&lua, "on_validate", &mut lua_ctx, script)
                .map_err(|e| DaemonError::Lua(format!("on_validate hook failed: {e}")))?;
            if lua_ctx.skip {
                tracing::info!(stage = %stage.id.raw, "skipped by on_validate hook");
                return Ok(());
            }
        }

        let mut messages = Vec::new();

        let mut system_prompt = String::new();
        if let Some(ref ctx) = ctx {
            if let Some(ref role) = ctx.frontmatter.role {
                system_prompt.push_str(role);
                system_prompt.push_str("\n\n");
            }
            if !ctx.body.is_empty() {
                system_prompt.push_str(&ctx.body);
            }
        }
        if !system_prompt.is_empty() {
            messages.push(Message::new(Role::System, system_prompt));
        }

        if let Some(ref ctx) = ctx {
            if !ctx.frontmatter.multimodal_input.is_empty() {
                for mm in &ctx.frontmatter.multimodal_input {
                    let path = if mm.path.starts_with('.') {
                        stage.path.join(&mm.path)
                    } else {
                        PathBuf::from(&mm.path)
                    };

                    match tokio::fs::read_to_string(&path).await {
                        Ok(data) => {
                            use base64::Engine;
                            let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
                            let media_type = match mm.input_type.as_str() {
                                "image" => {
                                    let ext = path.extension()
                                        .and_then(|e| e.to_str())
                                        .unwrap_or("png");
                                    format!("image/{ext}")
                                }
                                _ => "application/octet-stream".to_string(),
                            };
                            let url = format!("data:{media_type};base64,{encoded}");
                            messages.push(Message::with_content(
                                Role::User,
                                Content::ImageUrl {
                                    image_url: ImageUrlContent {
                                        url,
                                        detail: mm.detail.clone(),
                                    },
                                },
                            ));
                        }
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "skipping multimodal input file"
                            );
                        }
                    }
                }
            }
        }

        if !input_content.is_empty() {
            messages.push(Message::new(Role::User, input_content));
        } else if !messages.is_empty() {
            messages.push(Message::new(Role::User, "Execute the task described above."));
        }

        tracing::info!(
            workflow_id = workflow_id,
            stage = %stage.id.raw,
            model = %model,
            profile = %profile_name,
            messages = messages.len(),
            "calling LLM"
        );

        let response = llm
            .complete_with_profile(&config, &messages, None, profile.as_ref(), &stage_ctx)
            .await
            .map_err(|e| {
                tracing::error!(stage = %stage.id.raw, error = %e, "LLM call failed");
                DaemonError::Llm(format!("LLM call failed: {e}"))
            })?;

        let content = response.content.unwrap_or_default();

        tokio::fs::create_dir_all(&stage.output_path).await
            .map_err(|e| DaemonError::io(&stage.output_path, e))?;

        let result_path = stage.output_path.join("result.md");
        tokio::fs::write(&result_path, &content).await
            .map_err(|e| DaemonError::io(&result_path, e))?;

        if let Some(ref reasoning) = response.reasoning {
            if stage_ctx.capture_reasoning {
                let reasoning_path = stage.output_path.join("reasoning.md");
                tokio::fs::write(&reasoning_path, reasoning).await
                    .map_err(|e| DaemonError::io(&reasoning_path, e))?;
                tracing::info!(
                    stage = %stage.id.raw,
                    path = %reasoning_path.display(),
                    bytes = reasoning.len(),
                    "wrote reasoning output"
                );
            }
        }

        tracing::info!(
            stage = %stage.id.raw,
            path = %result_path.display(),
            bytes = content.len(),
            "wrote LLM output"
        );

        if let Some(ref script) = lua_script {
            let lua = create_sandboxed_vm()
                .map_err(|e| DaemonError::Lua(format!("Lua VM init failed: {e}")))?;
            let wf_root = self.workflows.get(workflow_id)
                .map(|wf| wf.root.clone())
                .unwrap_or_else(|| stage.path.clone());
            let mut lua_ctx = LuaContext::new(
                &stage.id.raw,
                &stage.path,
                &wf_root,
            )
            .with_output(&content, response.usage.completion_tokens as u64)
            .with_shared_store(shared_store.clone());
            if let Err(e) = run_hook(&lua, "on_complete", &mut lua_ctx, script) {
                tracing::warn!(stage = %stage.id.raw, error = %e, "on_complete hook failed");
            } else {
                if let Err(e) = save_shared_store(&wf_root, &shared_store) {
                    tracing::warn!(error = %e, "failed to save shared store");
                }
                for new_stage in &lua_ctx.hooks.insert_after {
                    if let Some(wf) = self.workflows.get_mut(workflow_id) {
                        if let Err(e) = wf.insert_stage_after(&stage.id.raw, new_stage).await {
                            tracing::warn!(stage = new_stage, error = %e, "failed to insert stage");
                        }
                    }
                }
                for remove_raw in &lua_ctx.hooks.remove_stages {
                    if let Some(wf) = self.workflows.get_mut(workflow_id) {
                        if let Err(e) = wf.remove_stage(remove_raw).await {
                            tracing::warn!(stage = remove_raw, error = %e, "failed to remove stage");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn create_llm(&self) -> Result<Box<dyn LLM>, DaemonError> {
        let provider_name =
            std::env::var("ZEROCHAIN_LLM_PROVIDER").unwrap_or_else(|_| "openai".into());
        let custom_base_url = std::env::var("ZEROCHAIN_BASE_URL").ok();
        let base_url = custom_base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1");
        let model = std::env::var("ZEROCHAIN_MODEL").unwrap_or_else(|_| "gpt-4o".into());

        let api_key_env = std::env::var("ZEROCHAIN_API_KEY_ENV")
            .unwrap_or_else(|_| "OPENAI_API_KEY".into());

        let provider = if custom_base_url.is_some() {
            ProviderId::OpenAICompatible {
                base_url: base_url.to_owned(),
                api_key_env,
            }
        } else {
            match provider_name.as_str() {
                "openai" => ProviderId::OpenAI,
                _ => ProviderId::OpenAICompatible {
                    base_url: base_url.to_owned(),
                    api_key_env: std::env::var("ZEROCHAIN_API_KEY_ENV")
                        .unwrap_or_else(|_| "OPENAI_API_KEY".into()),
                },
            }
        };

        let config = LLMConfig::new(provider, &model);
        LLMFactory::create(&config)
            .map_err(|e| DaemonError::Llm(format!("failed to create LLM provider: {e}")))
    }

    async fn read_input_files(&self, input_path: &Path) -> Result<String, DaemonError> {
        if !input_path.exists() {
            return Ok(String::new());
        }

        let mut entries = tokio::fs::read_dir(input_path).await
            .map_err(|e| DaemonError::io(input_path, e))?;
        let mut parts = Vec::new();

        while let Some(entry) = entries.next_entry().await
            .map_err(|e| DaemonError::io(input_path, e))? {
            let path = entry.path();
            if path.is_file() {
                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => {
                        if let Some(name) = path.file_name() {
                            parts.push(format!(
                                "--- {} ---\n{}",
                                name.to_string_lossy(),
                                content
                            ));
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "skipping non-text input file"
                        );
                    }
                }
            }
        }

        Ok(parts.join("\n\n"))
    }
}

fn copy_tree_stage<'a>(
    src: &'a Path,
    dst: &'a Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), DaemonError>> + Send + 'a>> {
    Box::pin(async move {
        let mut entries = tokio::fs::read_dir(src).await.map_err(|e| DaemonError::io(src, e))?;
        while let Some(entry) = entries.next_entry().await.map_err(|e| DaemonError::io(src, e))? {
            let file_name = entry.file_name();
            let src_path = entry.path();
            let dst_path = dst.join(&file_name);

            let file_type = entry.file_type().await.map_err(|e| DaemonError::io(&src_path, e))?;

            if file_type.is_dir() {
                tokio::fs::create_dir_all(&dst_path)
                    .await
                    .map_err(|e| DaemonError::io(&dst_path, e))?;
                copy_tree_stage(&src_path, &dst_path).await?;
            } else if file_type.is_file() {
                tokio::fs::copy(&src_path, &dst_path)
                    .await
                    .map_err(|e| DaemonError::io(&src_path, e))?;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn app_state_new_empty() {
        let tmp = TempDir::new().unwrap();
        let state = AppState::new(tmp.path());
        assert!(state.workflows.is_empty());
        assert_eq!(state.workspace_root, tmp.path());
    }

    #[test]
    fn workflow_dir_format() {
        let tmp = TempDir::new().unwrap();
        let dir = workflow_dir(tmp.path());
        assert_eq!(dir, tmp.path().join(".zerochain").join("workflows"));
    }

    #[test]
    fn parse_thinking_mode_variants() {
        assert!(matches!(parse_thinking_mode("disabled"), ThinkingMode::Disabled));
        assert!(matches!(parse_thinking_mode("extended"), ThinkingMode::Extended { .. }));
        assert!(matches!(
            parse_thinking_mode("extended:4096"),
            ThinkingMode::Extended { budget_tokens: 4096 }
        ));
        assert!(matches!(parse_thinking_mode("default"), ThinkingMode::Default));
        assert!(matches!(parse_thinking_mode(""), ThinkingMode::Default));
    }

    #[tokio::test]
    async fn init_workflow_creates_stages() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path());
        let wf = state.init_workflow(InitWorkflowParams { name: "test-wf", path: None, template: None }).await.unwrap();
        assert_eq!(wf.id, "test-wf");
        assert_eq!(wf.stages.len(), 3);
        assert!(state.get_workflow("test-wf").is_some());
    }

    #[tokio::test]
    async fn init_workflow_with_custom_template() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path());
        let wf = state.init_workflow(InitWorkflowParams { name: "custom", path: None, template: Some("01_a,02_b") }).await.unwrap();
        assert_eq!(wf.stages.len(), 2);
        assert_eq!(wf.stages[0].id.raw, "01_a");
        assert_eq!(wf.stages[1].id.raw, "02_b");
    }

    #[tokio::test]
    async fn list_workflows_returns_sorted() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path());
        state.init_workflow(InitWorkflowParams { name: "beta", path: None, template: None }).await.unwrap();
        state.init_workflow(InitWorkflowParams { name: "alpha", path: None, template: None }).await.unwrap();
        let list = state.list_workflows();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].0, "alpha");
        assert_eq!(list[1].0, "beta");
    }

    #[tokio::test]
    async fn mark_stage_complete_creates_marker() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path());
        let wf = state.init_workflow(InitWorkflowParams { name: "mark-test", path: None, template: Some("00_spec") }).await.unwrap();
        let stage = &wf.stages[0];

        state.mark_stage_complete("mark-test", &stage.id.raw).await.unwrap();

        assert!(stage.path.join(".complete").exists());
    }

    #[tokio::test]
    async fn mark_stage_error_creates_marker() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path());
        let wf = state.init_workflow(InitWorkflowParams { name: "err-test", path: None, template: Some("00_spec") }).await.unwrap();
        let stage = &wf.stages[0];

        state.mark_stage_error("err-test", &stage.id.raw, Some("bad output")).await.unwrap();

        let content = tokio::fs::read_to_string(stage.path.join(".error")).await.unwrap();
        assert_eq!(content, "bad output");
    }
}
