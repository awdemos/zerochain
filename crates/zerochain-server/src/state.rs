use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use zerochain_daemon::state::AppState;

/// Shared server state, Clone-able for axum's State extractor.
///
/// Inner AppState is behind `Arc<Mutex<...>>` because `execute_stage`
/// requires `&mut self`.
#[derive(Clone)]
pub struct ServerState {
    pub inner: Arc<Mutex<AppState>>,
    pub workspace: PathBuf,
}

impl ServerState {
    pub fn new(workspace: &std::path::Path) -> Self {
        let app_state = AppState::new(workspace);
        Self {
            inner: Arc::new(Mutex::new(app_state)),
            workspace: workspace.to_path_buf(),
        }
    }

    pub async fn refresh(&self) -> anyhow::Result<()> {
        let mut state = self.inner.lock().await;
        state.load_workflows().await?;
        Ok(())
    }
}
