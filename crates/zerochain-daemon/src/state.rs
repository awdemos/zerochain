use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use zerochain_core::stage::StageId;
use zerochain_core::task::Task;
use zerochain_core::workflow::Workflow;

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
}
