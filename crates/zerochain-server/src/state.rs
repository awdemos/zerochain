use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use zerochain_broker::memory::MemoryBroker;
use zerochain_cas::CasStore;
use zerochain_daemon::state::AppState;

/// Shared server state, Clone-able for axum's State extractor.
///
/// Inner AppState is behind `Arc<Mutex<...>>` because `execute_stage`
/// requires `&mut self`.
#[derive(Clone)]
pub struct ServerState {
    pub inner: Arc<Mutex<AppState>>,
    pub workspace: PathBuf,
    pub cas: Option<CasStore>,
    pub broker: Option<MemoryBroker>,
    pub api_key: Option<String>,
}

impl ServerState {
    pub fn new(workspace: &std::path::Path) -> Self {
        let app_state = AppState::new(workspace);
        Self {
            inner: Arc::new(Mutex::new(app_state)),
            workspace: workspace.to_path_buf(),
            cas: None,
            broker: None,
            api_key: None,
        }
    }

    pub fn with_cas(mut self, cas: CasStore) -> Self {
        self.cas = Some(cas);
        self
    }

    pub fn with_broker(mut self, broker: MemoryBroker) -> Self {
        self.broker = Some(broker);
        self
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        self.api_key = if key.is_empty() { None } else { Some(key) };
        self
    }

    pub async fn refresh(&self) -> anyhow::Result<()> {
        let mut state = self.inner.lock().await;
        state.load_workflows().await?;
        Ok(())
    }
}
