use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::Mutex;
use zerochain_cas::CasStore;
use zerochain_core::workflow::Workflow;

use crate::actor::WorkflowHandle;
use crate::error::DaemonError;
use crate::state::AppState;

pub struct WorkflowRegistry {
    workspace: PathBuf,
    cas: Mutex<Option<CasStore>>,
    handles: Mutex<HashMap<String, WorkflowHandle>>,
}

impl WorkflowRegistry {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            cas: Mutex::new(None),
            handles: Mutex::new(HashMap::new()),
        }
    }

    pub async fn set_cas(&self, cas: CasStore) {
        *self.cas.lock().await = Some(cas);
    }

    /// Get an existing workflow actor handle, or spawn a new one.
    ///
    /// The check-and-insert is serialized by `self.handles`, so concurrent
    /// requests for the same workflow cannot spawn duplicate actors.
    pub async fn get_or_create(&self, id: &str) -> Result<WorkflowHandle, DaemonError> {
        {
            let handles = self.handles.lock().await;
            if let Some(handle) = handles.get(id) {
                return Ok(handle.clone());
            }
        }

        let cas = self.cas.lock().await.clone();
        let mut state = AppState::new(&self.workspace, cas).await;
        state.load_workflows().await?;
        let handle = WorkflowHandle::spawn(state);

        {
            let mut handles = self.handles.lock().await;
            if let Some(existing) = handles.get(id) {
                return Ok(existing.clone());
            }
            handles.insert(id.to_string(), handle.clone());
        }

        Ok(handle)
    }

    pub async fn init_workflow(
        &self,
        name: String,
        template: Option<String>,
    ) -> Result<Workflow, DaemonError> {
        let handle = self.get_or_create(&name).await?;
        handle.init_workflow(name, template).await
    }

    pub async fn list_workflows(&self) -> Vec<(String, String)> {
        let handles = self.handles.lock().await;
        if handles.is_empty() {
            return Vec::new();
        }

        let mut results = Vec::new();
        for (id, handle) in handles.iter() {
            match handle.get_workflow(id.clone()).await {
                Some(wf) => {
                    let plan = wf.execution_plan();
                    let status = if plan.is_complete() { "complete" } else { "active" };
                    results.push((id.clone(), status.to_string()));
                }
                None => {
                    results.push((id.clone(), "unknown".to_string()));
                }
            }
        }
        results.sort_by(|a, b| a.0.cmp(&b.0));
        results
    }

    pub async fn load_all(&self) -> Result<(), DaemonError> {
        let cas = self.cas.lock().await.clone();
        let mut fresh = AppState::new(&self.workspace, cas).await;
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
        let registry = WorkflowRegistry::new(tmp.path().to_path_buf());
        registry.init_workflow("alpha".into(), None).await.unwrap();

        let fresh = WorkflowRegistry::new(tmp.path().to_path_buf());
        fresh.load_all().await.unwrap();

        let list = fresh.list_workflows().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "alpha");
    }

    #[tokio::test]
    async fn list_workflows_returns_all_disk_workflows() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = WorkflowRegistry::new(tmp.path().to_path_buf());
        registry.init_workflow("beta".into(), None).await.unwrap();
        registry.init_workflow("alpha".into(), None).await.unwrap();

        let list = registry.list_workflows().await;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].0, "alpha");
        assert_eq!(list[1].0, "beta");
    }
}
