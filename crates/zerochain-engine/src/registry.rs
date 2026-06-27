use std::collections::HashMap;
use std::path::PathBuf;

use zerochain_cas::CasStore;
use zerochain_core::workflow::Workflow;

use crate::actor::WorkflowHandle;
use crate::error::DaemonError;
use crate::state::AppState;

pub struct WorkflowRegistry {
    workspace: PathBuf,
    cas: Option<CasStore>,
    handles: HashMap<String, WorkflowHandle>,
}

impl WorkflowRegistry {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            cas: None,
            handles: HashMap::new(),
        }
    }

    pub fn set_cas(&mut self, cas: CasStore) {
        self.cas = Some(cas);
    }

    pub async fn get_or_create(&mut self, id: &str) -> Result<WorkflowHandle, DaemonError> {
        if let Some(handle) = self.handles.get(id) {
            return Ok(handle.clone());
        }
        let mut state = AppState::new(&self.workspace, self.cas.clone()).await;
        state.load_workflows().await?;
        let handle = WorkflowHandle::spawn(state);
        self.handles.insert(id.to_string(), handle.clone());
        Ok(handle)
    }

    pub async fn init_workflow(
        &mut self,
        name: String,
        template: Option<String>,
    ) -> Result<Workflow, DaemonError> {
        let handle = self.get_or_create(&name).await?;
        handle.init_workflow(name, template).await
    }

    pub async fn list_workflows(&self) -> Vec<(String, String)> {
        let mut state = AppState::new(&self.workspace, self.cas.clone()).await;
        if let Err(e) = state.load_workflows().await {
            tracing::warn!(error = %e, "failed to load workflows for list");
            return Vec::new();
        }
        let mut results = Vec::new();
        for wf in state.workflows.values() {
            let plan = wf.execution_plan();
            let status = if plan.is_complete() {
                "complete"
            } else {
                "active"
            };
            results.push((wf.id.clone(), status.to_string()));
        }
        results.sort_by(|a, b| a.0.cmp(&b.0));
        results
    }

    pub async fn load_all(&mut self) -> Result<(), DaemonError> {
        let mut fresh = AppState::new(&self.workspace, self.cas.clone()).await;
        fresh.load_workflows().await?;
        for id in fresh.workflows.keys().cloned().collect::<Vec<_>>() {
            self.get_or_create(&id).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_all_creates_handles_for_existing_workflows() {
        let tmp = tempfile::tempdir().unwrap();
        let mut registry = WorkflowRegistry::new(tmp.path().to_path_buf());
        registry.init_workflow("alpha".into(), None).await.unwrap();

        let mut fresh = WorkflowRegistry::new(tmp.path().to_path_buf());
        fresh.load_all().await.unwrap();

        let list = fresh.list_workflows().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "alpha");
    }

    #[tokio::test]
    async fn list_workflows_returns_all_disk_workflows() {
        let tmp = tempfile::tempdir().unwrap();
        let mut registry = WorkflowRegistry::new(tmp.path().to_path_buf());
        registry.init_workflow("beta".into(), None).await.unwrap();
        registry.init_workflow("alpha".into(), None).await.unwrap();

        let list = registry.list_workflows().await;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].0, "alpha");
        assert_eq!(list[1].0, "beta");
    }
}
