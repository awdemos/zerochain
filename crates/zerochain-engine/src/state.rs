use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::error::DaemonError;
use crate::llm_driver::LLMStageDriver;
use zerochain_cas::CasStore;
use zerochain_core::context::ContextCache;
use zerochain_core::graph::ControlOutcome;
use zerochain_core::stage::{Stage, StageId};
use zerochain_core::task::Task;
use zerochain_core::workflow::Workflow;
use zerochain_fs::{acquire_lock, clean_output, CowPlatform};
use zerochain_llm::{LLMConfig, LLMFactory, ProviderId, LLM};
use zerochain_tools::ToolRegistry;

/// Shared request type for HTTP and MCP entrypoints.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[non_exhaustive]
pub struct InitWorkflowRequest {
    pub name: String,
    #[serde(default)]
    pub template: Option<String>,
}

pub struct InitWorkflowParams<'a> {
    pub name: &'a str,
    pub path: Option<&'a Path>,
    pub template: Option<&'a str>,
    pub force: bool,
}

pub struct AppState {
    pub workspace_root: PathBuf,
    pub workflows: HashMap<String, Workflow>,
    pub cas: Option<CasStore>,
    pub tool_registry: Arc<ToolRegistry>,
    context_cache: ContextCache,
    cow_backend: Arc<dyn zerochain_fs::CowPlatform + Send + Sync>,
}

fn workflow_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".zerochain").join("workflows")
}

fn cow_backend_cache(
) -> &'static Mutex<HashMap<PathBuf, Arc<dyn zerochain_fs::CowPlatform + Send + Sync>>> {
    static CACHE: OnceLock<
        Mutex<HashMap<PathBuf, Arc<dyn zerochain_fs::CowPlatform + Send + Sync>>>,
    > = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[tracing::instrument(skip(workspace_root), fields(dir = %workspace_root.display()))]
async fn resolve_cow_backend(
    workspace_root: &Path,
) -> Arc<dyn zerochain_fs::CowPlatform + Send + Sync> {
    let cached = cow_backend_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(workspace_root).cloned());
    if let Some(backend) = cached {
        tracing::debug!(backend = %backend.name(), "using cached CoW backend");
        return backend;
    }

    let env_val = std::env::var("ZEROCHAIN_COW_BACKEND")
        .unwrap_or_default()
        .to_lowercase();

    let backend = match env_val.as_str() {
        "btrfs" => {
            let mode = zerochain_fs::SubvolumeMode::from_env();
            let btrfs = zerochain_fs::BtrfsCow::new(mode);
            if btrfs.is_available().await {
                tracing::info!("CoW backend: btrfs (forced by ZEROCHAIN_COW_BACKEND)");
                Arc::new(btrfs)
            } else {
                tracing::warn!(
                    "ZEROCHAIN_COW_BACKEND=btrfs but btrfs unavailable, falling back to auto"
                );
                zerochain_fs::detect_backend(workspace_root).await
            }
        }
        "directory" => {
            tracing::info!("CoW backend: directory (forced by ZEROCHAIN_COW_BACKEND)");
            Arc::new(zerochain_fs::DirectoryCow)
        }
        "none" | "disabled" => {
            tracing::info!("CoW backend: disabled by ZEROCHAIN_COW_BACKEND)");
            Arc::new(zerochain_fs::NoopCow)
        }
        _ => zerochain_fs::detect_backend(workspace_root).await,
    };

    if let Ok(mut cache) = cow_backend_cache().lock() {
        cache.insert(workspace_root.to_path_buf(), backend.clone());
    }
    backend
}

const MAX_SNAPSHOTS_PER_WORKFLOW: usize = 10;

impl AppState {
    #[tracing::instrument(skip(workspace_root, cas), fields(dir = %workspace_root.display()))]
    pub async fn new(workspace_root: &Path, cas: Option<CasStore>) -> AppState {
        let start = std::time::Instant::now();
        let cow_backend = resolve_cow_backend(workspace_root).await;
        tracing::info!(
            dir = %workspace_root.display(),
            backend = %cow_backend.name(),
            elapsed_ms = start.elapsed().as_millis(),
            "created AppState"
        );
        AppState {
            workspace_root: workspace_root.to_path_buf(),
            workflows: HashMap::new(),
            cas,
            tool_registry: Arc::new(ToolRegistry::default()),
            context_cache: ContextCache::default(),
            cow_backend,
        }
    }

    /// Load all workflows from the workspace workflows directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the workflow directory cannot be read or if any
    /// workflow definition fails to parse. Partial failures are reported via
    /// [`DaemonError::WorkflowLoadPartial`].
    #[tracing::instrument(skip(self), err, fields(dir = %self.workspace_root.display()))]
    pub async fn load_workflows(&mut self) -> Result<(), DaemonError> {
        let dir = workflow_dir(&self.workspace_root);
        match tokio::fs::metadata(&dir).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(DaemonError::io(&dir, e)),
        }

        let mut entries = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| DaemonError::io(&dir, e))?;
        let mut failures = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| DaemonError::io(&dir, e))?
        {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            match Workflow::from_dir(&path).await {
                Ok(wf) => {
                    self.workflows.insert(wf.id.clone(), wf);
                }
                Err(e) => {
                    let msg = format!("{}: {}", path.display(), e);
                    tracing::warn!(path = %path.display(), error = %e, "failed to load workflow");
                    failures.push(msg);
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(DaemonError::WorkflowLoadPartial(failures.join("; ")))
        }
    }

    #[must_use]
    pub fn get_workflow(&self, id: &str) -> Option<&Workflow> {
        self.workflows.get(id)
    }

    pub fn get_workflow_mut(&mut self, id: &str) -> Option<&mut Workflow> {
        self.workflows.get_mut(id)
    }

    pub async fn reload_workflow(&mut self, id: &str) -> Result<(), DaemonError> {
        let start = std::time::Instant::now();
        let wf = self
            .workflows
            .get(id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(id.into()))?;
        let root = wf.root.clone();
        let reloaded = Workflow::from_dir(&root).await?;
        self.workflows.insert(id.to_string(), reloaded);
        tracing::info!(
            workflow_id = id,
            elapsed_ms = start.elapsed().as_millis(),
            "reloaded workflow"
        );
        Ok(())
    }

    async fn refresh_single_stage(
        &mut self,
        workflow_id: &str,
        stage_raw: &str,
    ) -> Result<(), DaemonError> {
        let start = std::time::Instant::now();
        let wf = self
            .workflows
            .get_mut(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;
        wf.refresh_stage(stage_raw)
            .await
            .map_err(DaemonError::Workflow)?;
        tracing::info!(workflow_id, stage = %stage_raw, elapsed_ms = start.elapsed().as_millis(), "refreshed single stage");
        Ok(())
    }

    /// Run a single stage through its full lifecycle: acquire lock, clean output,
    /// execute, mark complete or error, and reload the workflow.
    #[tracing::instrument(skip(self), fields(workflow_id, stage_id = %stage_raw))]
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

        if stage.human_gate {
            return Err(DaemonError::Workflow(
                zerochain_core::error::Error::PlanError {
                    reason: format!(
                        "stage {} is waiting at a human gate; use approve or reject",
                        stage.id.raw
                    ),
                },
            ));
        }

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
                let outcome = self.detect_control_outcome(workflow_id, stage_raw).await;
                self.mark_stage_complete(workflow_id, stage_raw, outcome)
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "failed to mark stage complete after successful execution");
                        e
                    })?;
            }
            Err(e) => {
                let msg = format!("{e}");
                if let Err(e2) = self
                    .mark_stage_error(workflow_id, stage_raw, Some(&msg))
                    .await
                {
                    tracing::error!(original = %e, marker_error = %e2, "failed to mark stage error after execution failure");
                }
            }
        }

        if let Err(e) = self.refresh_single_stage(workflow_id, stage_raw).await {
            tracing::warn!(error = %e, "failed to refresh stage; falling back to full reload");
            if let Err(e) = self.reload_workflow(workflow_id).await {
                tracing::warn!(error = %e, "failed to reload workflow");
            }
        }

        result
    }

    /// Resolve the next pending stage and run it atomically.
    ///
    /// Returns `Ok(Some(stage_raw))` if a stage was executed, `Ok(None)` if no
    /// pending stages remain, or `Err` if the workflow/stage could not be run.
    pub async fn run_next_stage(
        &mut self,
        workflow_id: &str,
    ) -> Result<Option<String>, DaemonError> {
        let wf = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?
            .clone();
        let plan = wf.execution_plan();
        let next_stage = match plan.next_stage() {
            Some(stage) => stage.clone(),
            None => return Ok(None),
        };

        // If the plan selected a loop body for another iteration, clear the
        // previous completion marker so the body can run again.
        if plan.should_reset_body_for_loop_iteration(&next_stage) {
            let complete_marker = wf.root.join(&next_stage.raw).join(".complete");
            match tokio::fs::remove_file(&complete_marker).await {
                Ok(()) => {
                    tracing::debug!(
                        stage = %next_stage.raw,
                        "cleared loop body completion marker for next iteration"
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(
                        marker = %complete_marker.display(),
                        error = %e,
                        "failed to clear loop body completion marker"
                    );
                }
            }
        }

        self.run_stage(workflow_id, &next_stage.raw).await?;
        Ok(Some(next_stage.raw))
    }

    pub async fn init_workflow(
        &mut self,
        params: InitWorkflowParams<'_>,
    ) -> Result<Workflow, DaemonError> {
        let InitWorkflowParams {
            name,
            path,
            template,
            force,
        } = params;
        let base = path.unwrap_or(&self.workspace_root);
        let wf_base = workflow_dir(base);
        tokio::fs::create_dir_all(&wf_base)
            .await
            .map_err(|e| DaemonError::io(&wf_base, e))?;

        let sanitized_id = name
            .replace(['/', '\\'], "-")
            .replace("..", "-")
            .replace('\0', "");
        if sanitized_id.is_empty() || sanitized_id.len() > 128 {
            return Err(DaemonError::Workflow(
                zerochain_core::error::Error::InvalidWorkflowName {
                    name: name.to_string(),
                },
            ));
        }
        let workflow_root = wf_base.join(&sanitized_id);
        match tokio::fs::metadata(&workflow_root).await {
            Ok(_) => {
                if force {
                    tokio::fs::remove_dir_all(&workflow_root)
                        .await
                        .map_err(|e| DaemonError::io(&workflow_root, e))?;
                } else {
                    return Err(DaemonError::WorkflowExists {
                        name: name.to_string(),
                    });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(DaemonError::io(&workflow_root, e)),
        }

        let registry = zerochain_core::template::TemplateRegistry::new();
        let named_template = template.and_then(|t| registry.get(t));

        let (stage_names, stage_defs): (
            Vec<String>,
            Option<&Vec<zerochain_core::template::StageDef>>,
        ) = if let Some(tpl) = named_template {
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

        let cow_backend = self.cow_backend.clone();
        let workflow = Workflow::init_with_factories(
            &task,
            &wf_base,
            move |path| {
                let backend = cow_backend.clone();
                Box::pin(async move {
                    backend.prepare_workflow_root(&path).await.map_err(|e| {
                        zerochain_core::error::Error::PlanError {
                            reason: e.to_string(),
                        }
                    })
                })
            },
            {
                let backend = self.cow_backend.clone();
                move |path| {
                    let backend = backend.clone();
                    Box::pin(async move {
                        backend.prepare_stage_dir(&path).await.map_err(|e| {
                            zerochain_core::error::Error::PlanError {
                                reason: e.to_string(),
                            }
                        })
                    })
                }
            },
        )
        .await?;

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

    /// Read the stage's `output/result.md` and return any control record found
    /// at the start of the file.
    async fn detect_control_outcome(
        &self,
        workflow_id: &str,
        stage_id: &str,
    ) -> Option<ControlOutcome> {
        let wf = self.workflows.get(workflow_id)?;
        let sid = StageId::parse(stage_id).ok()?;
        let stage = wf.stage_by_id(&sid)?;
        let result_path = stage.output_path.join("result.md");
        let content = match tokio::fs::read_to_string(&result_path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!(path = %result_path.display(), error = %e, "failed to read result for control record");
                return None;
            }
        };

        let first_line = content.lines().next().unwrap_or("").trim();
        ControlOutcome::parse_record(first_line)
    }

    pub async fn mark_stage_complete(
        &mut self,
        workflow_id: &str,
        stage_id: &str,
        control_outcome: Option<ControlOutcome>,
    ) -> Result<(), DaemonError> {
        let start = std::time::Instant::now();
        let wf = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;
        let sid = StageId::parse(stage_id).map_err(|e| DaemonError::InvalidStageId {
            stage_id: stage_id.into(),
            source: e,
        })?;
        let stage = wf
            .stage_by_id(&sid)
            .ok_or_else(|| DaemonError::StageNotFound(stage_id.into()))?;

        let marker = stage.path.join(".complete");
        tokio::fs::write(&marker, "")
            .await
            .map_err(|e| DaemonError::io(&marker, e))?;
        let err_marker = stage.path.join(".error");
        match tokio::fs::metadata(&err_marker).await {
            Ok(_) => {
                tokio::fs::remove_file(&err_marker)
                    .await
                    .map_err(|e| DaemonError::io(&err_marker, e))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(DaemonError::io(&err_marker, e)),
        }

        if let Some(outcome) = control_outcome {
            let control_path = stage.path.join(".control");
            tokio::fs::write(&control_path, outcome.as_record())
                .await
                .map_err(|e| DaemonError::io(&control_path, e))?;
        }

        tracing::info!(
            workflow_id,
            stage_id,
            control_outcome = ?control_outcome,
            elapsed_ms = start.elapsed().as_millis(),
            "marked stage complete"
        );
        Ok(())
    }

    pub async fn mark_stage_error(
        &mut self,
        workflow_id: &str,
        stage_id: &str,
        feedback: Option<&str>,
    ) -> Result<(), DaemonError> {
        let start = std::time::Instant::now();
        let wf = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;
        let sid = StageId::parse(stage_id).map_err(|e| DaemonError::InvalidStageId {
            stage_id: stage_id.into(),
            source: e,
        })?;
        let stage = wf
            .stage_by_id(&sid)
            .ok_or_else(|| DaemonError::StageNotFound(stage_id.into()))?;

        let marker = stage.path.join(".error");
        tokio::fs::write(&marker, feedback.unwrap_or(""))
            .await
            .map_err(|e| DaemonError::io(&marker, e))?;
        tracing::info!(
            workflow_id,
            stage_id,
            elapsed_ms = start.elapsed().as_millis(),
            "marked stage error"
        );
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(workflow_id, stage_id))]
    pub async fn snapshot_stage(
        &self,
        workflow_id: &str,
        stage_id: &str,
    ) -> Result<PathBuf, DaemonError> {
        let span = tracing::Span::current();
        span.record("workflow_id", workflow_id);
        span.record("stage_id", stage_id);
        let wf = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;
        let sid = StageId::parse(stage_id).map_err(|e| DaemonError::InvalidStageId {
            stage_id: stage_id.into(),
            source: e,
        })?;
        let stage = wf
            .stage_by_id(&sid)
            .ok_or_else(|| DaemonError::StageNotFound(stage_id.into()))?;

        Self::snapshot_stage_at_path(
            workflow_id,
            stage_id,
            &stage.path,
            &wf.root,
            self.cow_backend.clone(),
            self.cas.as_ref(),
        )
        .await
    }

    async fn snapshot_stage_at_path(
        workflow_id: &str,
        stage_id: &str,
        stage_path: &Path,
        workflow_root: &Path,
        cow_backend: Arc<dyn zerochain_fs::CowPlatform + Send + Sync>,
        cas: Option<&CasStore>,
    ) -> Result<PathBuf, DaemonError> {
        let start = std::time::Instant::now();
        let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let stage_dir_name = stage_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| stage_id.into());
        let snap_name = format!("{}.{}.{nonce}", stage_dir_name, timestamp);
        let snapshots_dir = workflow_root.join(".snapshots");
        let snap_dir = snapshots_dir.join(&snap_name);

        tokio::fs::create_dir_all(&snapshots_dir)
            .await
            .map_err(|e| DaemonError::io(&snapshots_dir, e))?;

        cow_backend
            .snapshot(stage_path, &snap_dir)
            .await
            .map_err(|e| DaemonError::CowSnapshot {
                stage: stage_dir_name.clone(),
                source: e,
            })?;

        if let Some(c) = cas {
            tracing::debug!(workflow_id, stage_id, cas_metrics = ?c.metrics(), "snapshot metrics");
        }

        tracing::info!(
            workflow_id,
            stage_id,
            backend = %cow_backend.name(),
            snapshot = %snap_dir.display(),
            elapsed_ms = start.elapsed().as_millis(),
            "stage snapshot created"
        );

        Self::cleanup_old_snapshots_static(&snapshots_dir, cow_backend.as_ref()).await?;

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
        let stage = wf
            .stage_by_id(&sid)
            .ok_or_else(|| DaemonError::StageNotFound(stage_id.into()))?;

        let snapshots_dir = wf.root.join(".snapshots");
        match tokio::fs::metadata(&snapshots_dir).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(DaemonError::CowRestore {
                    stage: stage_id.into(),
                    source: zerochain_fs::error::FsError::MarkerFailed {
                        dir: snapshots_dir.clone(),
                        reason: "no snapshots directory".into(),
                    },
                });
            }
            Err(e) => return Err(DaemonError::io(&snapshots_dir, e)),
        }

        let latest = find_latest_snapshot(&snapshots_dir, &stage.id.raw)
            .await
            .map_err(|e| DaemonError::io(&snapshots_dir, e))?
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

        match tokio::fs::metadata(&stage.path).await {
            Ok(_) => {
                self.cow_backend
                    .remove_stage_dir(&stage.path)
                    .await
                    .map_err(|e| DaemonError::CowRestore {
                        stage: stage.id.raw.clone(),
                        source: e,
                    })?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(DaemonError::io(&stage.path, e)),
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

    async fn cleanup_old_snapshots_static(
        snapshots_dir: &Path,
        backend: &dyn zerochain_fs::CowPlatform,
    ) -> Result<(), DaemonError> {
        let mut rd = tokio::fs::read_dir(snapshots_dir)
            .await
            .map_err(|e| DaemonError::io(snapshots_dir, e))?;

        let mut names = Vec::new();
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| DaemonError::io(snapshots_dir, e))?
        {
            let file_type = entry
                .file_type()
                .await
                .map_err(|e| DaemonError::io(snapshots_dir, e))?;
            if !file_type.is_dir() {
                continue;
            }
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            names.push(name.to_string());
        }

        if names.len() <= MAX_SNAPSHOTS_PER_WORKFLOW {
            return Ok(());
        }

        // Snapshot names embed a UTC timestamp + nanosecond nonce, so
        // lexicographic order is chronological order within a stage. This
        // avoids unreliable filesystem mtimes that copy utilities may preserve.
        names.sort();
        let to_remove = names.len() - MAX_SNAPSHOTS_PER_WORKFLOW;

        for name in names.iter().take(to_remove) {
            let path = snapshots_dir.join(name);
            if let Err(e) = backend.remove_snapshot(&path).await {
                tracing::warn!(path = %path.display(), error = %e, "failed to remove old snapshot");
            }
        }

        Ok(())
    }

    #[must_use]
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
            self.execute_stage_with_llm(workflow_id, stage, llm.as_ref())
                .await
        }
    }

    #[tracing::instrument(skip(self, stage, llm), fields(workflow_id, stage_id = %stage.id.raw))]
    pub async fn execute_stage_with_llm(
        &mut self,
        workflow_id: &str,
        stage: &Stage,
        llm: &dyn LLM,
    ) -> Result<(), DaemonError> {
        let driver = LLMStageDriver {
            workflow_id,
            stage,
            llm,
            cas: self.cas.clone(),
            context_cache: Some(self.context_cache.clone()),
            tool_registry: self.tool_registry.clone(),
        };

        // Snapshot the stage in the background while the LLM request is in flight.
        // The stage directory is not mutated by the LLM path (writes go to
        // stage.output_path), so the copy and the network call can safely overlap.
        let workflow_root = self
            .workflows
            .get(workflow_id)
            .map(|wf| wf.root.clone())
            .or_else(|| stage.path.parent().map(|p| p.to_path_buf()))
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;
        let snapshot_task = {
            let workflow_id = workflow_id.to_string();
            let stage_id = stage.id.raw.clone();
            let stage_path = stage.path.clone();
            let cow_backend = self.cow_backend.clone();
            let cas = self.cas.clone();
            tokio::spawn(async move {
                Self::snapshot_stage_at_path(
                    &workflow_id,
                    &stage_id,
                    &stage_path,
                    &workflow_root,
                    cow_backend,
                    cas.as_ref(),
                )
                .await
            })
        };

        let output = driver.execute(&mut self.workflows).await;

        let snap_result = snapshot_task.await.map_err(|e| {
            DaemonError::Workflow(zerochain_core::error::Error::PlanError {
                reason: format!("snapshot task panicked: {e}"),
            })
        })?;

        if let Err(ref e) = snap_result {
            tracing::warn!(error = %e, "stage snapshot failed during LLM execution");
        }

        let output = output.map_err(|e| {
            tracing::error!(stage = %stage.id.raw, error = %e, "LLM call failed");
            if snap_result.is_ok() {
                tracing::info!(
                    stage = %stage.id.raw,
                    "snapshot available for restore via restore_stage()"
                );
            }
            e
        })?;

        if !output.is_empty() {
            driver.store_output_in_cas(&output).await?;
        }

        Ok(())
    }

    #[tracing::instrument(skip(self, stage), fields(workflow_id, stage_id = %stage.id.raw))]
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
        let wf = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| DaemonError::WorkflowNotFound(workflow_id.into()))?;

        tokio::fs::create_dir_all(&stage.output_path)
            .await
            .map_err(|e| DaemonError::io(&stage.output_path, e))?;

        let config = crate::container::ContainerConfig {
            image,
            stage_dir: stage.path.clone(),
            output_dir: stage.output_path.clone(),
            env_vars,
            command: vec![
                "zerochain".into(),
                "run".into(),
                workflow_id.into(),
                "--stage".into(),
                stage.id.raw.clone(),
            ],
            workspace_root: wf.root.clone(),
            workflow_id: workflow_id.into(),
            stage_id: stage.id.raw.clone(),
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

        let api_key_env =
            std::env::var("ZEROCHAIN_API_KEY_ENV").unwrap_or_else(|_| "OPENAI_API_KEY".into());

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
        LLMFactory::create(&config).map_err(DaemonError::Llm)
    }
}

async fn find_latest_snapshot(
    snapshots_dir: &Path,
    stage_id: &str,
) -> Result<Option<String>, std::io::Error> {
    let prefix = format!("{stage_id}.");
    let mut rd = tokio::fs::read_dir(snapshots_dir).await?;
    let mut candidates: Vec<String> = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        if !entry.file_type().await?.is_dir() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        candidates.push(name.to_string());
    }
    // Snapshot names embed a UTC timestamp + nanosecond nonce, so lexicographic
    // order is chronological order. Filesystem mtime is unreliable because copy
    // utilities preserve the source directory's modification time.
    candidates.sort();
    Ok(candidates.into_iter().last())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn app_state_new_empty() {
        let tmp = TempDir::new().unwrap();
        let state = AppState::new(tmp.path(), None).await;
        assert!(state.workflows.is_empty());
        assert_eq!(state.workspace_root, tmp.path());
    }

    #[test]
    fn workflow_dir_format() {
        let tmp = TempDir::new().unwrap();
        let dir = workflow_dir(tmp.path());
        assert_eq!(dir, tmp.path().join(".zerochain").join("workflows"));
    }

    #[tokio::test]
    async fn init_workflow_creates_stages() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        let wf = state
            .init_workflow(InitWorkflowParams {
                name: "test-wf",
                path: None,
                template: None,
                force: false,
            })
            .await
            .unwrap();
        assert_eq!(wf.id, "test-wf");
        assert_eq!(wf.stages.len(), 3);
        assert!(state.get_workflow("test-wf").is_some());
    }

    #[tokio::test]
    async fn init_workflow_with_custom_template() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        let wf = state
            .init_workflow(InitWorkflowParams {
                name: "custom",
                path: None,
                template: Some("01_a,02_b"),
                force: false,
            })
            .await
            .unwrap();
        assert_eq!(wf.stages.len(), 2);
        assert_eq!(wf.stages[0].id.raw, "01_a");
        assert_eq!(wf.stages[1].id.raw, "02_b");
    }

    #[tokio::test]
    async fn list_workflows_returns_sorted() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        state
            .init_workflow(InitWorkflowParams {
                name: "beta",
                path: None,
                template: None,
                force: false,
            })
            .await
            .unwrap();
        state
            .init_workflow(InitWorkflowParams {
                name: "alpha",
                path: None,
                template: None,
                force: false,
            })
            .await
            .unwrap();
        let list = state.list_workflows();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].0, "alpha");
        assert_eq!(list[1].0, "beta");
    }

    #[tokio::test]
    async fn mark_stage_complete_creates_marker() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        let wf = state
            .init_workflow(InitWorkflowParams {
                name: "mark-test",
                path: None,
                template: Some("00_spec"),
                force: false,
            })
            .await
            .unwrap();
        let stage = &wf.stages[0];

        state
            .mark_stage_complete("mark-test", &stage.id.raw, None)
            .await
            .unwrap();

        assert!(stage.path.join(".complete").exists());
    }

    #[tokio::test]
    async fn mark_stage_error_creates_marker() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        let wf = state
            .init_workflow(InitWorkflowParams {
                name: "err-test",
                path: None,
                template: Some("00_spec"),
                force: false,
            })
            .await
            .unwrap();
        let stage = &wf.stages[0];

        state
            .mark_stage_error("err-test", &stage.id.raw, Some("bad output"))
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(stage.path.join(".error"))
            .await
            .unwrap();
        assert_eq!(content, "bad output");
    }

    #[tokio::test]
    async fn snapshot_stage_creates_snapshot_directory() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        let wf = state
            .init_workflow(InitWorkflowParams {
                name: "snap-test",
                path: None,
                template: Some("00_spec"),
                force: false,
            })
            .await
            .unwrap();
        let stage = &wf.stages[0];

        tokio::fs::write(stage.path.join("data.txt"), b"original")
            .await
            .unwrap();

        let snap_path = state
            .snapshot_stage("snap-test", &stage.id.raw)
            .await
            .unwrap();

        assert!(snap_path.exists());
        let content = tokio::fs::read_to_string(snap_path.join("data.txt"))
            .await
            .unwrap();
        assert_eq!(content, "original");
    }

    #[tokio::test]
    async fn snapshot_stage_preserves_multiple_files() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        let wf = state
            .init_workflow(InitWorkflowParams {
                name: "multi-snap",
                path: None,
                template: Some("00_spec,01_impl"),
                force: false,
            })
            .await
            .unwrap();

        tokio::fs::write(wf.stages[0].path.join("a.txt"), b"aaa")
            .await
            .unwrap();
        tokio::fs::write(wf.stages[1].path.join("b.txt"), b"bbb")
            .await
            .unwrap();

        let snap0 = state
            .snapshot_stage("multi-snap", &wf.stages[0].id.raw)
            .await
            .unwrap();
        let snap1 = state
            .snapshot_stage("multi-snap", &wf.stages[1].id.raw)
            .await
            .unwrap();

        assert!(snap0.join("a.txt").exists());
        assert!(snap1.join("b.txt").exists());
    }

    #[tokio::test]
    async fn restore_stage_reverts_to_snapshot() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        let wf = state
            .init_workflow(InitWorkflowParams {
                name: "restore-test",
                path: None,
                template: Some("00_spec"),
                force: false,
            })
            .await
            .unwrap();
        let stage = &wf.stages[0];

        tokio::fs::write(stage.path.join("data.txt"), b"before")
            .await
            .unwrap();

        state
            .snapshot_stage("restore-test", &stage.id.raw)
            .await
            .unwrap();

        tokio::fs::write(stage.path.join("data.txt"), b"corrupted")
            .await
            .unwrap();
        tokio::fs::write(stage.path.join("extra.txt"), b"junk")
            .await
            .unwrap();

        state
            .restore_stage("restore-test", &stage.id.raw)
            .await
            .unwrap();

        let restored = tokio::fs::read_to_string(stage.path.join("data.txt"))
            .await
            .unwrap();
        assert_eq!(restored, "before");
        assert!(!stage.path.join("extra.txt").exists());
    }

    #[tokio::test]
    async fn restore_stage_fails_without_snapshot() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        let wf = state
            .init_workflow(InitWorkflowParams {
                name: "no-snap",
                path: None,
                template: Some("00_spec"),
                force: false,
            })
            .await
            .unwrap();

        let result = state.restore_stage("no-snap", &wf.stages[0].id.raw).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("no snapshot"),
            "unexpected error: {err_msg}"
        );
    }

    #[tokio::test]
    async fn snapshot_restores_latest_when_multiple() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        let wf = state
            .init_workflow(InitWorkflowParams {
                name: "multi-restore",
                path: None,
                template: Some("00_spec"),
                force: false,
            })
            .await
            .unwrap();
        let stage = &wf.stages[0];

        tokio::fs::write(stage.path.join("data.txt"), b"v1")
            .await
            .unwrap();
        state
            .snapshot_stage("multi-restore", &stage.id.raw)
            .await
            .unwrap();

        tokio::fs::write(stage.path.join("data.txt"), b"v2")
            .await
            .unwrap();
        state
            .snapshot_stage("multi-restore", &stage.id.raw)
            .await
            .unwrap();

        tokio::fs::write(stage.path.join("data.txt"), b"corrupted")
            .await
            .unwrap();

        state
            .restore_stage("multi-restore", &stage.id.raw)
            .await
            .unwrap();

        let restored = tokio::fs::read_to_string(stage.path.join("data.txt"))
            .await
            .unwrap();
        assert_eq!(restored, "v2");
    }

    #[tokio::test]
    async fn snapshot_cleanup_removes_oldest() {
        let tmp = TempDir::new().unwrap();
        let mut state = AppState::new(tmp.path(), None).await;
        let wf = state
            .init_workflow(InitWorkflowParams {
                name: "cleanup-test",
                path: None,
                template: Some("00_spec"),
                force: false,
            })
            .await
            .unwrap();
        let stage = &wf.stages[0];

        for i in 0..(MAX_SNAPSHOTS_PER_WORKFLOW + 3) {
            tokio::fs::write(stage.path.join("data.txt"), format!("v{i}"))
                .await
                .unwrap();
            state
                .snapshot_stage("cleanup-test", &stage.id.raw)
                .await
                .unwrap();
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

    #[tokio::test]
    async fn find_latest_snapshot_returns_none_on_empty() {
        let tmp = TempDir::new().unwrap();
        assert!(find_latest_snapshot(tmp.path(), "00_spec")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn find_latest_snapshot_picks_newest() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        for name in [
            "00_spec.20250101T000000Z",
            "00_spec.20250201T000000Z",
            "00_spec.20250301T000000Z",
        ] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let latest = find_latest_snapshot(dir, "00_spec").await.unwrap().unwrap();
        assert_eq!(latest, "00_spec.20250301T000000Z");
    }

    #[tokio::test]
    async fn find_latest_snapshot_ignores_other_stages() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("01_impl.20250101T000000Z")).unwrap();
        std::fs::create_dir_all(dir.join("00_spec.20250101T000000Z")).unwrap();

        let latest = find_latest_snapshot(dir, "00_spec").await.unwrap().unwrap();
        assert_eq!(latest, "00_spec.20250101T000000Z");
    }
}
