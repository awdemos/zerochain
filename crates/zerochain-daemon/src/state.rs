use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::DaemonError;
use zerochain_fs::{CowPlatform, acquire_lock, clean_output};
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
    cow_backend: Box<dyn zerochain_fs::CowPlatform>,
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
        .is_some_and(|s| s == "true" || s == "1")
}

fn resolve_cow_backend(workspace_root: &Path) -> Box<dyn zerochain_fs::CowPlatform> {
    let env_val = std::env::var("ZEROCHAIN_COW_BACKEND")
        .unwrap_or_default()
        .to_lowercase();

    match env_val.as_str() {
        "btrfs" => {
            let btrfs = zerochain_fs::BtrfsCow;
            if btrfs.is_available() {
                tracing::info!("CoW backend: btrfs (forced by ZEROCHAIN_COW_BACKEND)");
                return Box::new(btrfs);
            }
            tracing::warn!("ZEROCHAIN_COW_BACKEND=btrfs but btrfs unavailable, falling back to auto");
            zerochain_fs::detect_backend(workspace_root)
        }
        "directory" => {
            tracing::info!("CoW backend: directory (forced by ZEROCHAIN_COW_BACKEND)");
            Box::new(zerochain_fs::DirectoryCow)
        }
        "none" | "disabled" => {
            tracing::info!("CoW backend: disabled by ZEROCHAIN_COW_BACKEND");
            Box::new(zerochain_fs::NoopCow)
        }
        _ => zerochain_fs::detect_backend(workspace_root),
    }
}

const MAX_SNAPSHOTS_PER_WORKFLOW: usize = 10;

impl AppState {
    #[must_use] pub fn new(workspace_root: &Path) -> AppState {
        let cow_backend = resolve_cow_backend(workspace_root);
        AppState {
            workspace_root: workspace_root.to_path_buf(),
            workflows: HashMap::new(),
            cow_backend,
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

    #[must_use] pub fn get_workflow(&self, id: &str) -> Option<&Workflow> {
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

    /// Run a single stage through its full lifecycle: acquire lock, clean output,
    /// execute, mark complete or error, and reload the workflow.
    pub async fn run_stage(
        &mut self,
        workflow_id: &str,
        stage_raw: &str,
    ) -> Result<(), DaemonError> {
        let wf = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;
        let sid = StageId::parse(stage_raw).map_err(|e| DaemonError::InvalidStageId {
            stage_id: stage_raw.into(),
            source: e,
        })?;
        let stage = wf
            .stage_by_id(&sid)
            .ok_or_else(|| DaemonError::StageNotFound(stage_raw.into()))?
            .clone();

        let _lock = acquire_lock(&stage.path).await?;

        if let Err(e) = clean_output(&stage.path).await {
            tracing::error!(
                error = %e,
                path = %stage.path.display(),
                "failed to clean stage output"
            );
        }

        let result = self.execute_stage(workflow_id, &stage).await;

        match &result {
            Ok(()) => {
                self.mark_stage_complete(workflow_id, stage_raw).await.map_err(|e| {
                    tracing::error!(error = %e, "failed to mark stage complete after successful execution");
                    e
                })?;
            }
            Err(e) => {
                let msg = format!("{e}");
                if let Err(e2) = self.mark_stage_error(workflow_id, stage_raw, Some(&msg)).await {
                    tracing::error!(original = %e, marker_error = %e2, "failed to mark stage error after execution failure");
                }
            }
        }

        if let Err(e) = self.reload_workflow(workflow_id).await {
            tracing::warn!(error = %e, "failed to reload workflow");
        }

        result
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

        let registry = zerochain_core::template::TemplateRegistry::new();
        let named_template = template.and_then(|t| registry.get(t));

        let (stage_names, stage_defs): (Vec<String>, Option<&Vec<zerochain_core::template::StageDef>>) = if let Some(tpl) = named_template {
            (tpl.stage_names(), Some(&tpl.stages))
        } else {
            let names: Vec<String> = template.map_or_else(
                || vec!["00_spec".into(), "01_implement".into(), "02_verify".into()],
                |t| {
                    t.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                },
            );
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
                let ctx_path = workflow.root.join(&def.name).join("CONTEXT.md");
                tokio::fs::write(&ctx_path, def.to_context_md())
                    .await
                    .map_err(|e| DaemonError::io(&ctx_path, e))?;
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
        let sid = StageId::parse(stage_id).map_err(|e| DaemonError::InvalidStageId {
            stage_id: stage_id.into(),
            source: e,
        })?;
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
        let sid = StageId::parse(stage_id).map_err(|e| DaemonError::InvalidStageId {
            stage_id: stage_id.into(),
            source: e,
        })?;
        let stage = wf.stage_by_id(&sid).ok_or_else(|| DaemonError::StageNotFound(stage_id.into()))?;

        let marker = stage.path.join(".error");
        tokio::fs::write(&marker, feedback.unwrap_or(""))
            .await
            .map_err(|e| DaemonError::io(&marker, e))?;
        Ok(())
    }

    pub async fn snapshot_stage(
        &self,
        workflow_id: &str,
        stage_id: &str,
    ) -> Result<PathBuf, DaemonError> {
        let wf = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;
        let sid = StageId::parse(stage_id).map_err(|e| DaemonError::InvalidStageId {
            stage_id: stage_id.into(),
            source: e,
        })?;
        let stage = wf.stage_by_id(&sid).ok_or_else(|| DaemonError::StageNotFound(stage_id.into()))?;

        let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let snap_name = format!("{}.{}.{nonce}", &stage.id.raw, timestamp);
        let snapshots_dir = wf.root.join(".snapshots");
        let snap_dir = snapshots_dir.join(&snap_name);

        tokio::fs::create_dir_all(&snapshots_dir)
            .await
            .map_err(|e| DaemonError::io(&snapshots_dir, e))?;

        self.cow_backend
            .snapshot(&stage.path, &snap_dir)
            .await
            .map_err(|e| DaemonError::CowSnapshot {
                stage: stage.id.raw.clone(),
                source: e,
            })?;

        tracing::info!(
            stage = %stage.id.raw,
            backend = %self.cow_backend.name(),
            snapshot = %snap_dir.display(),
            "stage snapshot created"
        );

        self.cleanup_old_snapshots(&wf.root).await?;

        Ok(snap_dir)
    }

    pub async fn restore_stage(
        &self,
        workflow_id: &str,
        stage_id: &str,
    ) -> Result<(), DaemonError> {
        let wf = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;
        let sid = StageId::parse(stage_id).map_err(|e| DaemonError::InvalidStageId {
            stage_id: stage_id.into(),
            source: e,
        })?;
        let stage = wf.stage_by_id(&sid).ok_or_else(|| DaemonError::StageNotFound(stage_id.into()))?;

        let snapshots_dir = wf.root.join(".snapshots");
        if !snapshots_dir.exists() {
            return Err(DaemonError::CowRestore {
                stage: stage_id.into(),
                source: zerochain_fs::error::FsError::MarkerFailed {
                    dir: snapshots_dir.clone(),
                    reason: "no snapshots directory".into(),
                },
            });
        }

        let latest = find_latest_snapshot(&snapshots_dir, &stage.id.raw)
            .ok_or_else(|| DaemonError::CowRestore {
                stage: stage_id.into(),
                source: zerochain_fs::error::FsError::MarkerFailed {
                    dir: snapshots_dir.clone(),
                    reason: format!("no snapshot found for stage {stage_id}"),
                },
            })?;

        let snap_path = snapshots_dir.join(&latest);
        tracing::info!(
            stage = %stage.id.raw,
            snapshot = %snap_path.display(),
            "restoring stage from snapshot"
        );

        if stage.path.exists() {
            tokio::fs::remove_dir_all(&stage.path)
                .await
                .map_err(|e| DaemonError::io(&stage.path, e))?;
        }

        self.cow_backend
            .snapshot(&snap_path, &stage.path)
            .await
            .map_err(|e| DaemonError::CowRestore {
                stage: stage.id.raw.clone(),
                source: e,
            })?;

        tracing::info!(stage = %stage.id.raw, "stage restored from snapshot");
        Ok(())
    }

    async fn cleanup_old_snapshots(&self, workflow_root: &Path) -> Result<(), DaemonError> {
        let snapshots_dir = workflow_root.join(".snapshots");
        if !snapshots_dir.exists() {
            return Ok(());
        }

        let mut rd = tokio::fs::read_dir(&snapshots_dir)
            .await
            .map_err(|e| DaemonError::io(&snapshots_dir, e))?;

        let mut entries = Vec::new();
        while let Some(entry) = rd.next_entry().await.map_err(|e| DaemonError::io(&snapshots_dir, e))? {
            let path = entry.path();
            if path.is_dir() {
                entries.push(entry);
            }
        }

        if entries.len() <= MAX_SNAPSHOTS_PER_WORKFLOW {
            return Ok(());
        }

        entries.sort_by_key(tokio::fs::DirEntry::file_name);
        let to_remove = entries.len() - MAX_SNAPSHOTS_PER_WORKFLOW;

        for entry in entries.iter().take(to_remove) {
            let path = entry.path();
            if let Err(e) = tokio::fs::remove_dir_all(&path).await {
                tracing::warn!(path = %path.display(), error = %e, "failed to remove old snapshot");
            }
        }

        Ok(())
    }

    #[must_use] pub fn list_workflows(&self) -> Vec<(String, String)> {
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
        let container_mode = std::env::var("ZEROCHAIN_CONTAINER_ISOLATION")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        if container_mode {
            let snapshot_taken = self.snapshot_stage(workflow_id, &stage.id.raw).await.ok();
            let result = self.execute_stage_in_container(workflow_id, stage).await;
            if result.is_err() && snapshot_taken.is_some() {
                tracing::info!(
                    stage = %stage.id.raw,
                    "snapshot available for restore via restore_stage()"
                );
            }
            result
        } else {
            let llm = Self::create_llm()?;
            self.execute_stage_with_llm(workflow_id, stage, llm.as_ref()).await
        }
    }

    #[allow(clippy::too_many_lines)]
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
            .as_ref().map_or_else(|| "generic".to_string(), resolve_profile_name);

        let thinking_mode = ctx
            .as_ref()
            .map(resolve_thinking_mode)
            .unwrap_or_default();

        let capture_reasoning = ctx
            .as_ref()
            .is_some_and(resolve_capture_reasoning);

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

        profile.validate_config(&config, &stage_ctx).map_err(DaemonError::ProfileValidation)?;

        let shared_store = match self.workflows.get(workflow_id) {
            Some(wf) => load_shared_store(&wf.root),
            None => load_shared_store(Path::new(".")),
        };

        if let Some(ref script) = lua_script {
            let lua = create_sandboxed_vm()
                .map_err(DaemonError::Workflow)?;
            let mut lua_ctx = LuaContext::new(
                &stage.id.raw,
                &stage.path,
                &self.workflows.get(workflow_id).map_or_else(|| stage.path.clone(), |wf| wf.root.clone()),
            ).with_shared_store(shared_store.clone());
            run_hook(&lua, "on_validate", &mut lua_ctx, script)
                .map_err(DaemonError::Workflow)?;
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

        let snapshot_taken = self.snapshot_stage(workflow_id, &stage.id.raw).await.ok();

        let response = llm
            .complete_with_profile(&config, &messages, None, profile.as_ref(), &stage_ctx)
            .await
            .map_err(|e| {
                tracing::error!(stage = %stage.id.raw, error = %e, "LLM call failed");
                if snapshot_taken.is_some() {
                    tracing::info!(
                        stage = %stage.id.raw,
                        "snapshot available for restore via restore_stage()"
                    );
                }
                DaemonError::Llm(e)
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
                .map_err(DaemonError::Workflow)?;
            let wf_root = self.workflows.get(workflow_id).map_or_else(|| stage.path.clone(), |wf| wf.root.clone());
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

    pub async fn execute_stage_in_container(
        &mut self,
        workflow_id: &str,
        stage: &Stage,
    ) -> Result<(), DaemonError> {
        let executor = crate::container::ContainerExecutor::detect()
            .ok_or_else(|| DaemonError::ContainerRuntimeNotFound)?;

        let image = std::env::var("ZEROCHAIN_STAGE_IMAGE")
            .unwrap_or_else(|_| "cgr.dev/chainguard/wolfi-base:latest".into());

        let env_vars = Self::container_env_vars();
        let wf = self.workflows.get(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;

        tokio::fs::create_dir_all(&stage.output_path).await
            .map_err(|e| DaemonError::io(&stage.output_path, e))?;

        let config = crate::container::ContainerConfig {
            image,
            stage_dir: stage.path.clone(),
            output_dir: stage.output_path.clone(),
            env_vars,
            command: vec![
                "zerochain".into(),
                "run-stage".into(),
                "--workflow-id".into(),
                workflow_id.into(),
                "--stage-id".into(),
                stage.id.raw.clone(),
                "--workspace".into(),
                "/workspace".into(),
            ],
            workspace_root: wf.root.clone(),
        };

        let result = executor.run_stage(&config).await?;

        tokio::fs::write(stage.output_path.join("result.md"), &result.stdout)
            .await
            .map_err(|e| DaemonError::io(stage.output_path.join("result.md"), e))?;

        if !result.stderr.is_empty() {
            let stderr_path = stage.output_path.join("stderr.log");
            tokio::fs::write(&stderr_path, &result.stderr)
                .await
                .map_err(|e| DaemonError::io(&stderr_path, e))?;
        }

        Ok(())
    }

    fn container_env_vars() -> Vec<(String, String)> {
        let mut vars = Vec::new();
        for key in &[
            "OPENAI_API_KEY",
            "ZEROCHAIN_MODEL",
            "ZEROCHAIN_LLM_PROVIDER",
            "ZEROCHAIN_BASE_URL",
            "ZEROCHAIN_API_KEY_ENV",
            "ZEROCHAIN_PROVIDER_PROFILE",
            "ZEROCHAIN_THINKING_MODE",
            "ZEROCHAIN_CAPTURE_REASONING",
        ] {
            if let Ok(val) = std::env::var(key) {
                vars.push(((*key).to_string(), val));
            }
        }
        vars
    }

    fn create_llm() -> Result<Box<dyn LLM>, DaemonError> {
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
            .map_err(DaemonError::Llm)
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

fn find_latest_snapshot(snapshots_dir: &Path, stage_id: &str) -> Option<String> {
    let prefix = format!("{stage_id}.");
    let Ok(entries) = std::fs::read_dir(snapshots_dir) else {
        return None;
    };
    let mut candidates: Vec<String> = entries
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .filter(|name| name.starts_with(&prefix))
        .collect();
    candidates.sort();
    candidates.into_iter().last()
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

    #[tokio::test]
    async fn snapshot_stage_creates_snapshot_directory() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path());
        let wf = state.init_workflow(InitWorkflowParams {
            name: "snap-test", path: None, template: Some("00_spec"),
        }).await.unwrap();
        let stage = &wf.stages[0];

        tokio::fs::write(stage.path.join("data.txt"), b"original")
            .await.unwrap();

        let snap_path = state.snapshot_stage("snap-test", &stage.id.raw).await.unwrap();

        assert!(snap_path.exists());
        let content = tokio::fs::read_to_string(snap_path.join("data.txt")).await.unwrap();
        assert_eq!(content, "original");
    }

    #[tokio::test]
    async fn snapshot_stage_preserves_multiple_files() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path());
        let wf = state.init_workflow(InitWorkflowParams {
            name: "multi-snap", path: None, template: Some("00_spec,01_impl"),
        }).await.unwrap();

        tokio::fs::write(wf.stages[0].path.join("a.txt"), b"aaa").await.unwrap();
        tokio::fs::write(wf.stages[1].path.join("b.txt"), b"bbb").await.unwrap();

        let snap0 = state.snapshot_stage("multi-snap", &wf.stages[0].id.raw).await.unwrap();
        let snap1 = state.snapshot_stage("multi-snap", &wf.stages[1].id.raw).await.unwrap();

        assert!(snap0.join("a.txt").exists());
        assert!(snap1.join("b.txt").exists());
    }

    #[tokio::test]
    async fn restore_stage_reverts_to_snapshot() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path());
        let wf = state.init_workflow(InitWorkflowParams {
            name: "restore-test", path: None, template: Some("00_spec"),
        }).await.unwrap();
        let stage = &wf.stages[0];

        tokio::fs::write(stage.path.join("data.txt"), b"before")
            .await.unwrap();

        state.snapshot_stage("restore-test", &stage.id.raw).await.unwrap();

        tokio::fs::write(stage.path.join("data.txt"), b"corrupted")
            .await.unwrap();
        tokio::fs::write(stage.path.join("extra.txt"), b"junk")
            .await.unwrap();

        state.restore_stage("restore-test", &stage.id.raw).await.unwrap();

        let restored = tokio::fs::read_to_string(stage.path.join("data.txt")).await.unwrap();
        assert_eq!(restored, "before");
        assert!(!stage.path.join("extra.txt").exists());
    }

    #[tokio::test]
    async fn restore_stage_fails_without_snapshot() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path());
        let wf = state.init_workflow(InitWorkflowParams {
            name: "no-snap", path: None, template: Some("00_spec"),
        }).await.unwrap();

        let result = state.restore_stage("no-snap", &wf.stages[0].id.raw).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("no snapshot"), "unexpected error: {err_msg}");
    }

    #[tokio::test]
    async fn snapshot_restores_latest_when_multiple() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path());
        let wf = state.init_workflow(InitWorkflowParams {
            name: "multi-restore", path: None, template: Some("00_spec"),
        }).await.unwrap();
        let stage = &wf.stages[0];

        tokio::fs::write(stage.path.join("data.txt"), b"v1").await.unwrap();
        state.snapshot_stage("multi-restore", &stage.id.raw).await.unwrap();

        tokio::fs::write(stage.path.join("data.txt"), b"v2").await.unwrap();
        state.snapshot_stage("multi-restore", &stage.id.raw).await.unwrap();

        tokio::fs::write(stage.path.join("data.txt"), b"corrupted").await.unwrap();

        state.restore_stage("multi-restore", &stage.id.raw).await.unwrap();

        let restored = tokio::fs::read_to_string(stage.path.join("data.txt")).await.unwrap();
        assert_eq!(restored, "v2");
    }

    #[tokio::test]
    async fn snapshot_cleanup_removes_oldest() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path());
        let wf = state.init_workflow(InitWorkflowParams {
            name: "cleanup-test", path: None, template: Some("00_spec"),
        }).await.unwrap();
        let stage = &wf.stages[0];

        for i in 0..(MAX_SNAPSHOTS_PER_WORKFLOW + 3) {
            tokio::fs::write(stage.path.join("data.txt"), format!("v{i}")).await.unwrap();
            state.snapshot_stage("cleanup-test", &stage.id.raw).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let snapshots_dir = wf.root.join(".snapshots");
        let mut rd = tokio::fs::read_dir(&snapshots_dir).await.unwrap();
        let mut count = 0;
        while rd.next_entry().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, MAX_SNAPSHOTS_PER_WORKFLOW);
    }

    #[test]
    fn find_latest_snapshot_returns_none_on_empty() {
        let tmp = TempDir::new().unwrap();
        assert!(find_latest_snapshot(tmp.path(), "00_spec").is_none());
    }

    #[test]
    fn find_latest_snapshot_picks_newest() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("00_spec.20250101T000000Z")).unwrap();
        std::fs::create_dir_all(dir.join("00_spec.20250201T000000Z")).unwrap();
        std::fs::create_dir_all(dir.join("00_spec.20250301T000000Z")).unwrap();

        let latest = find_latest_snapshot(dir, "00_spec").unwrap();
        assert_eq!(latest, "00_spec.20250301T000000Z");
    }

    #[test]
    fn find_latest_snapshot_ignores_other_stages() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("01_impl.20250101T000000Z")).unwrap();
        std::fs::create_dir_all(dir.join("00_spec.20250101T000000Z")).unwrap();

        let latest = find_latest_snapshot(dir, "00_spec").unwrap();
        assert_eq!(latest, "00_spec.20250101T000000Z");
    }
}
