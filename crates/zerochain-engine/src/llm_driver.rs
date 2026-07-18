use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;
use zerochain_cas::{CasStore, Cid};
use zerochain_core::context::{Context as StageContext, ContextCache};
use zerochain_core::stage::Stage;
use zerochain_core::workflow::Workflow;
use zerochain_core::{
    acquire_sandboxed_vm, load_shared_store, run_hook, save_shared_store, LuaContext, PooledLua,
};
use zerochain_llm::{
    resolve_profile, Content, ImageUrlContent, LLMConfig, Message, ProviderId, Role,
    StageContext as LlmStageContext, ThinkingMode, LLM,
};
use zerochain_memory::chunk_text;
use zerochain_tools::ToolRegistry;

use crate::error::DaemonError;
use crate::state::AppState;
use crate::tool_driver;

pub struct LLMStageDriver<'a> {
    pub workflow_id: &'a str,
    pub stage: &'a Stage,
    pub llm: &'a dyn LLM,
    pub cas: Option<CasStore>,
    pub context_cache: Option<ContextCache>,
    pub tool_registry: Arc<ToolRegistry>,
    pub state: Arc<AppState>,
}

impl<'a> LLMStageDriver<'a> {
    #[tracing::instrument(skip(self, workflows), fields(workflow_id, stage_id = %self.stage.id.raw))]
    pub async fn execute(
        &self,
        workflows: &mut HashMap<String, Workflow>,
    ) -> Result<String, DaemonError> {
        let has_context = tokio::fs::metadata(&self.stage.context_path).await.is_ok();
        let ctx = if has_context {
            Some(
                StageContext::from_md_file_cached(
                    &self.stage.context_path,
                    self.context_cache.as_ref(),
                )
                .await?,
            )
        } else {
            None
        };

        let lua_path = self.stage.path.join("CONTEXT.lua");
        let lua_script = match tokio::fs::metadata(&lua_path).await {
            Ok(_) => Some(
                tokio::fs::read_to_string(&lua_path)
                    .await
                    .map_err(|e| DaemonError::io(&lua_path, e))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(DaemonError::io(&lua_path, e)),
        };

        let input_content = read_input_files(&self.stage.input_path).await?;

        let profile_name = ctx
            .as_ref()
            .map_or_else(|| "generic".to_string(), resolve_profile_name);

        let thinking_mode = ctx.as_ref().map(resolve_thinking_mode).unwrap_or_default();

        let capture_reasoning = ctx.as_ref().is_some_and(resolve_capture_reasoning);

        let profile = resolve_profile(&profile_name);

        let model = std::env::var("ZEROCHAIN_MODEL").unwrap_or_else(|_| "gpt-4o".into());
        let mut config = LLMConfig::new(ProviderId::OpenAI, &model);

        if profile_name == "kimi-k2"
            || matches!(
                &thinking_mode,
                ThinkingMode::Disabled | ThinkingMode::Extended { .. }
            )
        {
            config = config.with_temperature(1.0);
        }

        let stage_ctx = LlmStageContext {
            thinking_mode,
            capture_reasoning,
        };

        profile
            .validate_config(&config, &stage_ctx)
            .map_err(DaemonError::ProfileValidation)?;

        let workflow_root = workflows
            .get(self.workflow_id)
            .map_or_else(|| self.stage.path.clone(), |wf| wf.root.clone());
        let memory_dir = workflow_root.join(".zerochain").join("memory");
        let shared_store = load_shared_store_async(workflow_root.clone()).await?;

        if let Some(ref script) = lua_script {
            let lua = acquire_sandboxed_vm().map_err(DaemonError::Workflow)?;
            let lua_ctx = LuaContext::new(&self.stage.id.raw, &self.stage.path, &workflow_root)
                .with_shared_store(shared_store.clone());
            let lua_ctx = run_hook_async(lua, "on_validate", lua_ctx, script.clone()).await?;
            if lua_ctx.skip {
                tracing::info!(stage = %self.stage.id.raw, "skipped by on_validate hook");
                return Ok(String::new());
            }
        }

        self.load_memory_sources(&ctx).await?;

        let mut messages = assemble_messages(&ctx, &input_content, self.stage).await;

        let tool_names = ctx
            .as_ref()
            .map(|c| c.frontmatter.tools.as_slice())
            .unwrap_or(&[]);
        let tools_vec = tool_driver::to_llm_tools(&self.tool_registry, tool_names);
        let tools = if tools_vec.is_empty() {
            None
        } else {
            Some(tools_vec.as_slice())
        };

        tracing::info!(
            workflow_id = self.workflow_id,
            stage = %self.stage.id.raw,
            model = %model,
            profile = %profile_name,
            messages = messages.len(),
            "calling LLM"
        );

        let max_iterations = ctx
            .as_ref()
            .and_then(|c| c.frontmatter.tool_loop_max_iterations)
            .unwrap_or(10);

        let mut tool_round = 0;

        let (output, response) = loop {
            let response = self
                .llm
                .complete_with_profile(&config, &messages, tools, profile.as_ref(), &stage_ctx)
                .await
                .map_err(DaemonError::Llm)?;

            if response.tool_calls.is_empty() {
                write_stage_output(self.stage, &response, &stage_ctx).await?;
                let output = response.content.clone().unwrap_or_default();
                break (output, response);
            }

            tool_round += 1;
            // Exceeded the configured number of tool-use iterations; execute the
            // remaining tool calls and write their raw results as the stage output.
            if tool_round >= max_iterations {
                let mut results = Vec::new();
                for call in &response.tool_calls {
                    let result = tool_driver::execute_tool_call(
                        &self.tool_registry,
                        call,
                        &workflow_root,
                        Some(&memory_dir),
                    )
                    .await?;
                    results.push(format!("{}: {}", call.name, result));
                }
                let tool_output = results.join("\n\n");

                tokio::fs::create_dir_all(&self.stage.output_path)
                    .await
                    .map_err(|e| DaemonError::io(&self.stage.output_path, e))?;

                let result_path = self.stage.output_path.join("result.md");
                tokio::fs::write(&result_path, &tool_output)
                    .await
                    .map_err(|e| DaemonError::io(&result_path, e))?;
                tracing::info!(
                    stage = %self.stage.id.raw,
                    path = %result_path.display(),
                    bytes = tool_output.len(),
                    iterations = tool_round,
                    "wrote tool call output after exhausting tool loop"
                );
                break (tool_output, response);
            }

            for call in &response.tool_calls {
                let result = tool_driver::execute_tool_call(
                    &self.tool_registry,
                    call,
                    &workflow_root,
                    Some(&memory_dir),
                )
                .await?;
                let result_text = format!(
                    "Tool result for call {} ({}): {}",
                    call.id, call.name, result
                );
                messages.push(Message::new(Role::Tool, result_text));
            }
        };

        self.index_output(&ctx, &output).await?;

        if let Some(ref script) = lua_script {
            run_post_completion_hooks(
                workflows,
                self.workflow_id,
                self.stage,
                script,
                &output,
                response.usage.completion_tokens as u64,
                &shared_store,
            )
            .await?;
        }

        Ok(output)
    }

    pub async fn store_output_in_cas(&self, output: &str) -> Result<Option<Cid>, DaemonError> {
        if let Some(ref cas) = self.cas {
            let cid = cas.put(output.as_bytes()).await.map_err(DaemonError::Cas)?;
            tracing::info!(
                stage = %self.stage.id.raw,
                cid = %cid,
                bytes = output.len(),
                "stored stage output in CAS"
            );
            Ok(Some(cid))
        } else {
            Ok(None)
        }
    }
}

impl<'a> LLMStageDriver<'a> {
    async fn load_memory_sources(&self, ctx: &Option<StageContext>) -> Result<(), DaemonError> {
        let Some(model) = self.state.embedding_model.as_ref() else {
            return Ok(());
        };
        let Some(ctx) = ctx else {
            return Ok(());
        };
        if ctx.frontmatter.memory_sources.is_empty() {
            return Ok(());
        }

        let workflow_root = self.state.workflow_root(self.workflow_id).await?;
        let mut parts = Vec::new();
        let mut source_paths = Vec::new();
        for src in &ctx.frontmatter.memory_sources {
            let path = workflow_root.join(src);
            match tokio::fs::metadata(&path).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(DaemonError::io(&path, e)),
            }
            let content = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| DaemonError::io(&path, e))?;
            parts.push(content);
            source_paths.push(path);
        }
        if parts.is_empty() {
            return Ok(());
        }

        let combined = parts.join("\n\n");
        let chunk_size = ctx.frontmatter.memory_chunk_size.unwrap_or(1000);
        let overlap = ctx.frontmatter.memory_chunk_overlap.unwrap_or(200);
        let chunks = chunk_text(&combined, chunk_size, overlap);
        if chunks.is_empty() {
            return Ok(());
        }

        let store = self.state.workflow_memory_store(self.workflow_id).await?;
        let mut locked = store.lock().await;
        let texts: Vec<(String, serde_json::Value)> = chunks
            .into_iter()
            .zip(source_paths.iter().cycle())
            .map(|(chunk, path)| (chunk, json!({ "source": path.display().to_string() })))
            .collect();
        locked.add(&**model, texts).await?;
        Ok(())
    }

    async fn index_output(
        &self,
        ctx: &Option<StageContext>,
        output: &str,
    ) -> Result<(), DaemonError> {
        let Some(model) = self.state.embedding_model.as_ref() else {
            return Ok(());
        };
        let Some(ctx) = ctx else {
            return Ok(());
        };
        if !ctx.frontmatter.index_output {
            return Ok(());
        }
        if output.is_empty() {
            return Ok(());
        }

        let chunk_size = ctx.frontmatter.memory_chunk_size.unwrap_or(1000);
        let overlap = ctx.frontmatter.memory_chunk_overlap.unwrap_or(200);
        let chunks = chunk_text(output, chunk_size, overlap);
        if chunks.is_empty() {
            return Ok(());
        }

        let store = self.state.workflow_memory_store(self.workflow_id).await?;
        let mut locked = store.lock().await;
        let stage_id = self.stage.id.raw.clone();
        let texts: Vec<(String, serde_json::Value)> = chunks
            .into_iter()
            .map(|chunk| (chunk, json!({ "stage": stage_id })))
            .collect();
        locked.add(&**model, texts).await?;
        Ok(())
    }
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
            ThinkingMode::Extended {
                budget_tokens: budget,
            }
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
        .is_some_and(|s| s == "true" || s == "1")
}

async fn read_input_files(input_path: &Path) -> Result<String, DaemonError> {
    let start = std::time::Instant::now();
    match tokio::fs::metadata(input_path).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(DaemonError::io(input_path, e)),
    }

    let mut entries = tokio::fs::read_dir(input_path)
        .await
        .map_err(|e| DaemonError::io(input_path, e))?;
    let mut parts = Vec::new();
    let mut files_read = 0usize;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| DaemonError::io(input_path, e))?
    {
        let path = entry.path();
        let is_file = match entry.metadata().await {
            Ok(m) => m.is_file(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping entry with unreadable metadata");
                continue;
            }
        };
        if !is_file {
            continue;
        }
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => {
                if let Some(name) = path.file_name() {
                    parts.push(format!("--- {} ---\n{}", name.to_string_lossy(), content));
                    files_read += 1;
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

    tracing::info!(
        path = %input_path.display(),
        files_read,
        elapsed_ms = start.elapsed().as_millis(),
        "read stage input files"
    );
    Ok(parts.join("\n\n"))
}

async fn assemble_messages(
    ctx: &Option<StageContext>,
    input_content: &str,
    stage: &Stage,
) -> Vec<Message> {
    let start = std::time::Instant::now();
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
                                let ext =
                                    path.extension().and_then(|e| e.to_str()).unwrap_or("png");
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
        messages.push(Message::new(Role::User, input_content.to_string()));
    } else if !messages.is_empty() {
        messages.push(Message::new(
            Role::User,
            "Execute the task described above.",
        ));
    }

    tracing::info!(
        stage = %stage.id.raw,
        messages = messages.len(),
        elapsed_ms = start.elapsed().as_millis(),
        "assembled LLM messages"
    );

    messages
}

async fn write_stage_output(
    stage: &Stage,
    response: &zerochain_llm::CompleteResponse,
    stage_ctx: &LlmStageContext,
) -> Result<PathBuf, DaemonError> {
    let start = std::time::Instant::now();
    let content = response.content.clone().unwrap_or_default();

    tokio::fs::create_dir_all(&stage.output_path)
        .await
        .map_err(|e| DaemonError::io(&stage.output_path, e))?;

    let result_path = stage.output_path.join("result.md");
    tokio::fs::write(&result_path, &content)
        .await
        .map_err(|e| DaemonError::io(&result_path, e))?;

    if let Some(ref reasoning) = response.reasoning {
        if stage_ctx.capture_reasoning {
            let reasoning_path = stage.output_path.join("reasoning.md");
            tokio::fs::write(&reasoning_path, reasoning)
                .await
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
        elapsed_ms = start.elapsed().as_millis(),
        "wrote LLM output"
    );

    Ok(result_path)
}

async fn run_hook_async(
    lua: PooledLua,
    hook_name: &'static str,
    mut ctx: LuaContext,
    script: String,
) -> Result<LuaContext, DaemonError> {
    tokio::task::spawn_blocking(move || {
        run_hook(lua.get(), hook_name, &mut ctx, &script).map_err(DaemonError::Workflow)?;
        Ok(ctx)
    })
    .await
    .map_err(|e| {
        DaemonError::Workflow(zerochain_core::error::Error::PlanError {
            reason: format!("hook task panicked: {e}"),
        })
    })?
}

async fn load_shared_store_async(
    workflow_root: PathBuf,
) -> Result<Arc<Mutex<HashMap<String, serde_json::Value>>>, DaemonError> {
    tokio::task::spawn_blocking(move || {
        load_shared_store(&workflow_root).map_err(DaemonError::Workflow)
    })
    .await
    .map_err(|e| {
        DaemonError::Workflow(zerochain_core::error::Error::PlanError {
            reason: format!("shared store load task panicked: {e}"),
        })
    })?
}

async fn save_shared_store_async(
    workflow_root: PathBuf,
    store: Arc<Mutex<HashMap<String, serde_json::Value>>>,
) -> Result<(), DaemonError> {
    tokio::task::spawn_blocking(move || {
        save_shared_store(&workflow_root, &store).map_err(DaemonError::Workflow)
    })
    .await
    .map_err(|e| {
        DaemonError::Workflow(zerochain_core::error::Error::PlanError {
            reason: format!("shared store save task panicked: {e}"),
        })
    })?
}

async fn run_post_completion_hooks(
    workflows: &mut HashMap<String, Workflow>,
    workflow_id: &str,
    stage: &Stage,
    script: &str,
    content: &str,
    completion_tokens: u64,
    shared_store: &Arc<Mutex<HashMap<String, serde_json::Value>>>,
) -> Result<(), DaemonError> {
    let start = std::time::Instant::now();
    let lua = acquire_sandboxed_vm().map_err(DaemonError::Workflow)?;
    let wf_root = workflows
        .get(workflow_id)
        .map_or_else(|| stage.path.clone(), |wf| wf.root.clone());
    let lua_ctx = LuaContext::new(&stage.id.raw, &stage.path, &wf_root)
        .with_output(content, completion_tokens)
        .with_shared_store(shared_store.clone());
    match run_hook_async(lua, "on_complete", lua_ctx, script.to_string()).await {
        Ok(lua_ctx) => {
            if let Err(e) = save_shared_store_async(wf_root.clone(), shared_store.clone()).await {
                tracing::warn!(error = %e, "failed to save shared store");
            }
            for new_stage in &lua_ctx.hooks.insert_after {
                if let Some(wf) = workflows.get_mut(workflow_id) {
                    if let Err(e) = wf.insert_stage_after(&stage.id.raw, new_stage).await {
                        tracing::warn!(stage = new_stage, error = %e, "failed to insert stage");
                    }
                }
            }
            for remove_raw in &lua_ctx.hooks.remove_stages {
                if let Some(wf) = workflows.get_mut(workflow_id) {
                    if let Err(e) = wf.remove_stage(remove_raw).await {
                        tracing::warn!(stage = remove_raw, error = %e, "failed to remove stage");
                    }
                }
            }
            tracing::info!(
                stage = %stage.id.raw,
                inserted = lua_ctx.hooks.insert_after.len(),
                removed = lua_ctx.hooks.remove_stages.len(),
                elapsed_ms = start.elapsed().as_millis(),
                "ran post-completion hooks"
            );
        }
        Err(e) => {
            tracing::warn!(stage = %stage.id.raw, error = %e, "on_complete hook failed");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tempfile::TempDir;
    use zerochain_core::stage::Stage;
    use zerochain_llm::error::LLMError;
    use zerochain_llm::types::{CompleteResponse, LLMConfig, Message, ProviderId, Tool};
    use zerochain_memory::{EmbeddingModel, MemoryError};

    struct FakeLlm {
        response: String,
    }

    #[async_trait]
    impl LLM for FakeLlm {
        fn provider_id(&self) -> &ProviderId {
            static PROVIDER: ProviderId = ProviderId::OpenAI;
            &PROVIDER
        }

        async fn complete(
            &self,
            _config: &LLMConfig,
            _messages: &[Message],
            _tools: Option<&[Tool]>,
        ) -> std::result::Result<CompleteResponse, LLMError> {
            Ok(CompleteResponse::new(Some(self.response.clone())))
        }

        fn supports_multimodal(&self) -> bool {
            false
        }

        fn context_window(&self) -> usize {
            128_000
        }

        async fn health_check(&self) -> std::result::Result<(), LLMError> {
            Ok(())
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct FakeEmbed;

    #[async_trait]
    impl EmbeddingModel for FakeEmbed {
        async fn embed(&self, _texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, MemoryError> {
            Ok(_texts.iter().map(|_| vec![1.0f32, 0.0, 0.0]).collect())
        }
    }

    async fn test_state_with_embedding(tmp: &TempDir) -> Arc<AppState> {
        let mut state = AppState::new(tmp.path(), None).await;
        state.embedding_model = Some(Arc::new(FakeEmbed));
        Arc::new(state)
    }

    #[tokio::test]
    async fn indexes_stage_output_when_index_output_is_true() {
        let tmp = TempDir::new().unwrap();
        let state = test_state_with_embedding(&tmp).await;
        let mut state_mut = state.clone_state();
        let wf = state_mut
            .init_workflow(crate::state::InitWorkflowParams {
                name: "idx-test",
                path: None,
                template: Some("00_spec"),
                force: false,
            })
            .await
            .unwrap();
        let state = Arc::new(state_mut);
        let ctx_path = wf.root.join("00_spec").join("CONTEXT.md");
        tokio::fs::write(
            &ctx_path,
            "---\nindex_output: true\n---\nSummarize the task.",
        )
        .await
        .unwrap();

        let stage = Stage::from_dir(&wf.root.join("00_spec")).await.unwrap();
        let llm = FakeLlm {
            response: "The quick brown fox jumps over the lazy dog.".into(),
        };
        let mut workflows = HashMap::new();
        workflows.insert(wf.id.clone(), wf);
        let driver = LLMStageDriver {
            workflow_id: "idx-test",
            stage: &stage,
            llm: &llm,
            cas: None,
            context_cache: None,
            tool_registry: Arc::new(ToolRegistry::default()),
            state: state.clone(),
        };

        let output = driver.execute(&mut workflows).await.unwrap();
        assert_eq!(output, "The quick brown fox jumps over the lazy dog.");

        let store = state.workflow_memory_store("idx-test").await.unwrap();
        let locked = store.lock().await;
        let model = state.embedding_model.as_ref().unwrap();
        let results = locked
            .query(model.as_ref(), "quick brown fox", 1)
            .await
            .unwrap();
        assert!(
            !results.is_empty(),
            "expected indexed output to be searchable"
        );
    }

    #[tokio::test]
    async fn preloads_memory_sources_before_llm_call() {
        let tmp = TempDir::new().unwrap();
        let state = test_state_with_embedding(&tmp).await;
        let mut state_mut = state.clone_state();
        let wf = state_mut
            .init_workflow(crate::state::InitWorkflowParams {
                name: "mem-test",
                path: None,
                template: Some("00_spec"),
                force: false,
            })
            .await
            .unwrap();
        let state = Arc::new(state_mut);

        let docs_dir = wf.root.join("docs");
        tokio::fs::create_dir_all(&docs_dir).await.unwrap();
        let readme = docs_dir.join("readme.md");
        tokio::fs::write(
            &readme,
            "# Project\n\nThis project uses vector memory for semantic search.",
        )
        .await
        .unwrap();

        let ctx_path = wf.root.join("00_spec").join("CONTEXT.md");
        tokio::fs::write(
            &ctx_path,
            "---\nmemory_sources:\n  - docs/readme.md\n---\nEcho the docs.",
        )
        .await
        .unwrap();

        let stage = Stage::from_dir(&wf.root.join("00_spec")).await.unwrap();
        let llm = FakeLlm {
            response: "The project uses vector memory.".into(),
        };
        let mut workflows = HashMap::new();
        workflows.insert(wf.id.clone(), wf);
        let driver = LLMStageDriver {
            workflow_id: "mem-test",
            stage: &stage,
            llm: &llm,
            cas: None,
            context_cache: None,
            tool_registry: Arc::new(ToolRegistry::default()),
            state: state.clone(),
        };

        let output = driver.execute(&mut workflows).await.unwrap();
        assert_eq!(output, "The project uses vector memory.");

        let store = state.workflow_memory_store("mem-test").await.unwrap();
        let locked = store.lock().await;
        let model = state.embedding_model.as_ref().unwrap();
        let results = locked
            .query(model.as_ref(), "vector memory", 1)
            .await
            .unwrap();
        assert!(
            !results.is_empty(),
            "expected memory source to be searchable"
        );
    }

    #[test]
    fn parse_thinking_mode_variants() {
        assert!(matches!(
            parse_thinking_mode("disabled"),
            ThinkingMode::Disabled
        ));
        assert!(matches!(
            parse_thinking_mode("extended"),
            ThinkingMode::Extended { .. }
        ));
        assert!(matches!(
            parse_thinking_mode("extended:4096"),
            ThinkingMode::Extended {
                budget_tokens: 4096
            }
        ));
        assert!(matches!(
            parse_thinking_mode("default"),
            ThinkingMode::Default
        ));
        assert!(matches!(parse_thinking_mode(""), ThinkingMode::Default));
    }
}
