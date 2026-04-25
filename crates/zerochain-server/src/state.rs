use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use zerochain_broker::Broker;
use zerochain_cas::CasStore;
use zerochain_engine::{AppState, DaemonError};

/// Shared server state, Clone-able for axum's State extractor.
///
/// Inner `AppState` is behind `Arc<Mutex<...>>` because `execute_stage`
/// requires `&mut self`.
#[derive(Clone)]
pub struct ServerState {
    pub inner: Arc<Mutex<AppState>>,
    pub workspace: PathBuf,
    pub cas: Option<CasStore>,
    pub broker: Option<Arc<dyn Broker>>,
    pub api_key: Option<String>,
    pub auth_disabled: bool,
}

impl ServerState {
    #[must_use] pub fn new(workspace: &std::path::Path) -> Self {
        let app_state = AppState::new(workspace);
        Self {
            inner: Arc::new(Mutex::new(app_state)),
            workspace: workspace.to_path_buf(),
            cas: None,
            broker: None,
            api_key: None,
            auth_disabled: false,
        }
    }

    #[must_use] pub fn with_cas(mut self, cas: CasStore) -> Self {
        self.cas = Some(cas);
        self
    }

    #[must_use] pub fn with_broker(mut self, broker: Arc<dyn Broker>) -> Self {
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
        let mut state = self.inner.lock().await;
        state.load_workflows().await?;
        Ok(())
    }

    #[must_use] pub fn cas(&self) -> Option<&CasStore> {
        self.cas.as_ref()
    }

    #[must_use] pub fn broker(&self) -> Option<&Arc<dyn Broker>> {
        self.broker.as_ref()
    }
}
