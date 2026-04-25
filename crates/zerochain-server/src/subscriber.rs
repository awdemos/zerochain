//! Background broker subscriber that bridges cross-pod messages into the
//! filesystem-native workflow model.
//!
//! On startup, spawns a tokio task that subscribes to `zerochain.*.*`.
//! When a message arrives, fetches the prompt content from CAS by CID and
//! writes it into the target stage's `input/` directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::fs;
use tracing;
use zerochain_broker::{Broker, BrokerMessage};
use zerochain_cas::CasStore;

/// Spawn the background subscriber task.
///
/// Runs until the broker subscription ends or an unrecoverable error occurs.
pub async fn spawn(
    cas: CasStore,
    broker: Arc<dyn Broker>,
    workspace: PathBuf,
) {
    tracing::info!("starting background broker subscriber");

    let subject = "zerochain.*.*";
    let mut rx = match broker.subscribe(subject).await {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!(error = %e, subject, "failed to subscribe to broker");
            return;
        }
    };

    tracing::info!(subject, "subscribed to broker");

    while let Some(msg) = rx.recv().await {
        if let Err(e) = handle_message(&cas, &workspace, &msg).await {
            tracing::warn!(
                workflow_id = %msg.workflow_id,
                from_stage = %msg.from_stage,
                to_stage = %msg.to_stage,
                error = %e,
                "failed to handle broker message"
            );
        }
    }

    tracing::info!("background broker subscriber ended");
}

async fn handle_message(
    cas: &CasStore,
    workspace: &Path,
    msg: &BrokerMessage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Fetch prompt content from CAS.
    let content = cas.get(&msg.prompt_cid).await.map_err(|e| {
        tracing::warn!(
            cid = %msg.prompt_cid,
            error = %e,
            "failed to fetch prompt from CAS"
        );
        e
    })?;

    // Build target path: {workspace}/.zerochain/workflows/{workflow_id}/{to_stage}/input/{from_stage}.md
    let input_dir = workspace
        .join(".zerochain")
        .join("workflows")
        .join(&msg.workflow_id)
        .join(&msg.to_stage)
        .join("input");

    fs::create_dir_all(&input_dir).await.map_err(|e| {
        tracing::warn!(path = %input_dir.display(), error = %e, "failed to create input directory");
        e
    })?;

    let file_name = format!("{}.md", msg.from_stage);
    let input_path = input_dir.join(&file_name);

    fs::write(&input_path, &content).await.map_err(|e| {
        tracing::warn!(path = %input_path.display(), error = %e, "failed to write input file");
        e
    })?;

    tracing::info!(
        workflow_id = %msg.workflow_id,
        from_stage = %msg.from_stage,
        to_stage = %msg.to_stage,
        cid = %msg.prompt_cid,
        path = %input_path.display(),
        bytes = content.len(),
        "bridged broker message to stage input"
    );

    Ok(())
}
