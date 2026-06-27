use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use zerochain_broker::Broker;
use zerochain_cas::CasStore;
use zerochain_engine::{DaemonError, WorkflowRegistry};

/// Shared server state, Clone-able for axum's State extractor.
///
/// Workflows are managed by `WorkflowRegistry` behind `Arc<RwLock<...>>`.
#[derive(Clone)]
pub struct ServerState {
    pub registry: Arc<RwLock<WorkflowRegistry>>,
    pub workspace: PathBuf,
    pub cas: Option<CasStore>,
    pub broker: Option<Arc<dyn Broker>>,
    pub api_key: Option<String>,
    pub auth_disabled: bool,
}

impl ServerState {
    pub async fn new(workspace: &std::path::Path) -> Self {
        let registry = WorkflowRegistry::new(workspace.to_path_buf());
        Self {
            registry: Arc::new(RwLock::new(registry)),
            workspace: workspace.to_path_buf(),
            cas: None,
            broker: None,
            api_key: None,
            auth_disabled: false,
        }
    }

    #[must_use]
    pub fn with_cas(mut self, cas: CasStore) -> Self {
        self.cas = Some(cas);
        self
    }

    #[must_use]
    pub fn with_broker(mut self, broker: Arc<dyn Broker>) -> Self {
        self.broker = Some(broker);
        self
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        self.api_key = if key.is_empty() { None } else { Some(key) };
        self
    }

    #[must_use]
    pub fn with_auth_disabled(mut self) -> Self {
        self.auth_disabled = true;
        self
    }

    /// # Errors
    ///
    /// Returns `DaemonError` if workflow loading fails.
    pub async fn refresh(&self) -> Result<(), DaemonError> {
        let mut registry = self.registry.write().await;
        registry.load_all().await?;
        Ok(())
    }

    #[must_use]
    pub fn cas(&self) -> Option<&CasStore> {
        self.cas.as_ref()
    }

    #[must_use]
    pub fn broker(&self) -> Option<&Arc<dyn Broker>> {
        self.broker.as_ref()
    }
}
