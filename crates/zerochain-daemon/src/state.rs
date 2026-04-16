use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use zerochain_core::context::Context as StageContext;
use zerochain_core::stage::{Stage, StageId};
use zerochain_core::task::Task;
use zerochain_core::workflow::Workflow;
use zerochain_llm::{
    LLM, LLMConfig, LLMFactory, Message, ProviderId, Role,
};

pub struct AppState {
    pub workspace_root: PathBuf,
    pub workflows: HashMap<String, Workflow>,
}

fn workflow_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".zerochain").join("workflows")
}

impl AppState {
    pub fn new(workspace_root: &Path) -> AppState {
        AppState {
            workspace_root: workspace_root.to_path_buf(),
            workflows: HashMap::new(),
        }
    }

    pub async fn load_workflows(&mut self) -> anyhow::Result<()> {
        let dir = workflow_dir(&self.workspace_root);
        if !dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            match Workflow::from_dir(&path).await {
                Ok(wf) => {
                    self.workflows.insert(wf.id.clone(), wf);
                }
                Err(_) => continue,
            }
        }
        Ok(())
    }

    pub fn get_workflow(&self, id: &str) -> Option<&Workflow> {
        self.workflows.get(id)
    }

    pub async fn init_workflow(
        &mut self,
        path: Option<&Path>,
        name: &str,
        template: Option<&str>,
    ) -> anyhow::Result<Workflow> {
        let base = path.unwrap_or(&self.workspace_root);
        let wf_base = workflow_dir(base);
        tokio::fs::create_dir_all(&wf_base)
            .await
            .with_context(|| format!("creating workflow dir: {}", wf_base.display()))?;

        let stage_names: Vec<String> = template
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_else(|| vec!["00_spec".into(), "01_implement".into(), "02_verify".into()]);

        let task = Task::new(
            name.to_string(),
            name.to_string(),
            "todo".into(),
            None,
            Some(zerochain_core::task::TaskExecution::new(
                stage_names,
                Some("sequential".into()),
            )),
            vec![],
            String::new(),
            None,
        );

        let workflow = Workflow::init(&task, &wf_base).await?;
        self.workflows.insert(workflow.id.clone(), workflow.clone());
        Ok(workflow)
    }

    pub async fn mark_stage_complete(
        &mut self,
        workflow_id: &str,
        stage_id: &str,
    ) -> anyhow::Result<()> {
        let wf = self
            .workflows
            .get(workflow_id)
            .context("workflow not found")?;
        let sid = StageId::parse(stage_id).map_err(|e| anyhow::anyhow!("{e}"))?;
        let stage = wf.stage_by_id(&sid).context("stage not found")?;

        let marker = stage.path.join(".complete");
        tokio::fs::write(&marker, "").await?;
        let err_marker = stage.path.join(".error");
        if err_marker.exists() {
            tokio::fs::remove_file(err_marker).await?;
        }
        Ok(())
    }

    pub async fn mark_stage_error(
        &mut self,
        workflow_id: &str,
        stage_id: &str,
        feedback: Option<&str>,
    ) -> anyhow::Result<()> {
        let wf = self
            .workflows
            .get(workflow_id)
            .context("workflow not found")?;
        let sid = StageId::parse(stage_id).map_err(|e| anyhow::anyhow!("{e}"))?;
        let stage = wf.stage_by_id(&sid).context("stage not found")?;

        let marker = stage.path.join(".error");
        tokio::fs::write(&marker, feedback.unwrap_or(""))
            .await?;
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
        &self,
        workflow_id: &str,
        stage: &Stage,
    ) -> anyhow::Result<()> {
        let llm = self.create_llm()?;
        self.execute_stage_with_llm(workflow_id, stage, llm.as_ref()).await
    }

    /// Execute a stage using an injected LLM (for testing).
    pub async fn execute_stage_with_llm(
        &self,
        workflow_id: &str,
        stage: &Stage,
        llm: &dyn LLM,
    ) -> anyhow::Result<()> {
        let ctx = if stage.context_path.exists() {
            Some(StageContext::from_file(&stage.context_path).await?)
        } else {
            None
        };

        let input_content = self.read_input_files(&stage.input_path).await?;

        let model = std::env::var("ZEROCHAIN_MODEL").unwrap_or_else(|_| "gpt-4o".into());
        let config = LLMConfig::new(ProviderId::OpenAI, &model);

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

        if !input_content.is_empty() {
            messages.push(Message::new(Role::User, input_content));
        }

        tracing::info!(
            workflow_id = workflow_id,
            stage = %stage.id.raw,
            model = %model,
            messages = messages.len(),
            "calling LLM"
        );

        let response = llm.complete(&config, &messages, None).await.map_err(|e| {
            tracing::error!(stage = %stage.id.raw, error = %e, "LLM call failed");
            anyhow::anyhow!("LLM call failed: {e}")
        })?;

        let content = response.content.unwrap_or_default();

        tokio::fs::create_dir_all(&stage.output_path).await?;

        let result_path = stage.output_path.join("result.md");
        tokio::fs::write(&result_path, &content).await?;

        tracing::info!(
            stage = %stage.id.raw,
            path = %result_path.display(),
            bytes = content.len(),
            "wrote LLM output"
        );

        Ok(())
    }

    fn create_llm(&self) -> anyhow::Result<Box<dyn LLM>> {
        let provider_name =
            std::env::var("ZEROCHAIN_LLM_PROVIDER").unwrap_or_else(|_| "openai".into());
        let base_url =
            std::env::var("ZEROCHAIN_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
            anyhow::anyhow!("OPENAI_API_KEY environment variable is required")
        })?;
        let model = std::env::var("ZEROCHAIN_MODEL").unwrap_or_else(|_| "gpt-4o".into());

        let provider = match provider_name.as_str() {
            "openai" => ProviderId::OpenAI,
            _ => ProviderId::OpenAICompatible {
                base_url,
                api_key_env: "OPENAI_API_KEY".into(),
            },
        };

        let config = LLMConfig::new(provider, &model);
        std::env::set_var("OPENAI_API_KEY", &api_key);
        LLMFactory::create(&config)
            .map_err(|e| anyhow::anyhow!("failed to create LLM provider: {e}"))
    }

    async fn read_input_files(&self, input_path: &Path) -> anyhow::Result<String> {
        if !input_path.exists() {
            return Ok(String::new());
        }

        let mut entries = tokio::fs::read_dir(input_path).await?;
        let mut parts = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
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
