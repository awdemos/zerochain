use std::collections::{HashMap, HashSet};
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
        let state = AppState::new(&self.workspace, self.cas.clone()).await;
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
        let handles: Vec<_> = self.handles.values().cloned().collect();
        let mut results = Vec::new();
        for handle in handles {
            results.extend(handle.list_workflows().await);
        }
        let mut seen = HashSet::new();
        results
            .into_iter()
            .filter(|(id, _)| seen.insert(id.clone()))
            .collect()
    }

    pub async fn load_all(&mut self) -> Result<(), DaemonError> {
        let handles: Vec<_> = self.handles.values().cloned().collect();
        for handle in handles {
            if let Err(e) = handle.load_workflows().await {
                tracing::warn!(error = %e, "failed to load workflows for handle");
            }
        }
        Ok(())
    }
}
