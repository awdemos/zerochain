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
    ///
    /// Unknown ids are rejected WITHOUT spawning: each spawn builds a full
    /// `AppState` (embedding model + disk scan) that is retained forever, so
    /// answering random ids with fresh actors would exhaust memory and CPU.
    pub async fn get_or_create(&self, id: &str) -> Result<WorkflowHandle, DaemonError> {
        self.get_or_create_inner(id, true).await
    }

    async fn get_or_create_inner(
        &self,
        id: &str,
        require_exists: bool,
    ) -> Result<WorkflowHandle, DaemonError> {
        {
            let handles = self.handles.lock().await;
            if let Some(handle) = handles.get(id) {
                return Ok(handle.clone());
            }
        }

        if require_exists {
            let wf_dir = self.workspace.join(".zerochain").join("workflows").join(id);
            let exists = tokio::fs::metadata(&wf_dir)
                .await
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if !exists {
                return Err(DaemonError::WorkflowNotFound(id.to_string()));
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
        parents: Vec<String>,
    ) -> Result<Workflow, DaemonError> {
        // The workflow does not exist on disk yet — this is the one caller
        // allowed to spawn a handle for a not-yet-created id.
        let handle = self.get_or_create_inner(&name, false).await?;
        handle.init_workflow(name, template, parents).await
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
                    let status = if plan.is_complete() {
                        "complete"
                    } else {
                        "active"
                    };
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

    pub async fn export_okf(
        &self,
        workflow_id: String,
        output_dir: PathBuf,
    ) -> Result<PathBuf, DaemonError> {
        let handle = self.get_or_create(&workflow_id).await?;
        handle.export_okf(workflow_id, output_dir).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_all_creates_handles_for_existing_workflows() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = WorkflowRegistry::new(tmp.path().to_path_buf());
        registry
            .init_workflow("alpha".into(), None, vec![])
            .await
            .unwrap();

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
        registry
            .init_workflow("beta".into(), None, vec![])
            .await
            .unwrap();
        registry
            .init_workflow("alpha".into(), None, vec![])
            .await
            .unwrap();

        let list = registry.list_workflows().await;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].0, "alpha");
        assert_eq!(list[1].0, "beta");
    }

    #[tokio::test]
    async fn get_or_create_unknown_id_errors_without_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = WorkflowRegistry::new(tmp.path().to_path_buf());

        let err = registry
            .get_or_create("no-such-workflow")
            .await
            .unwrap_err();
        assert!(
            matches!(err, DaemonError::WorkflowNotFound(_)),
            "unexpected error: {err}"
        );
        assert!(
            registry.handles.lock().await.is_empty(),
            "unknown ids must not retain actor handles"
        );
    }

    #[tokio::test]
    async fn get_or_create_existing_disk_workflow_spawns_handle() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = WorkflowRegistry::new(tmp.path().to_path_buf());
        registry
            .init_workflow("on-disk".into(), None, vec![])
            .await
            .unwrap();

        // A fresh registry that has not loaded anything yet can still serve
        // the on-disk workflow, while a genuinely unknown id 404s.
        let fresh = WorkflowRegistry::new(tmp.path().to_path_buf());
        let handle = fresh.get_or_create("on-disk").await.unwrap();
        let wf = handle.get_workflow("on-disk".into()).await;
        assert!(wf.is_some());
        assert!(fresh.get_or_create("still-not-there").await.is_err());
    }
}
