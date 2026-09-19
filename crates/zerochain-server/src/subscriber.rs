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
pub async fn spawn(cas: CasStore, broker: Arc<dyn Broker>, workspace: PathBuf) {
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

/// A broker message field is safe to embed in
/// `{workspace}/.zerochain/workflows/{workflow_id}/{to_stage}/input/{from_stage}.md`
/// only if it is a single relative path component: non-empty, not absolute,
/// and free of separators and parent-directory references. This is a second
/// line of defense — the HTTP handler validates before publishing, but
/// messages can also arrive from an external NATS server.
fn is_safe_component(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('/')
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains("..")
}

async fn handle_message(
    cas: &CasStore,
    workspace: &Path,
    msg: &BrokerMessage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !is_safe_component(&msg.workflow_id)
        || !is_safe_component(&msg.to_stage)
        || !is_safe_component(&msg.from_stage)
    {
        tracing::warn!(
            workflow_id = %msg.workflow_id,
            from_stage = %msg.from_stage,
            to_stage = %msg.to_stage,
            "rejecting broker message with unsafe path components"
        );
        return Err("unsafe path components in broker message".into());
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_components_accept_stage_and_workflow_names() {
        assert!(is_safe_component("wf-1"));
        assert!(is_safe_component("v1.2"));
        assert!(is_safe_component("02_next"));
        assert!(is_safe_component("00_spec"));
    }

    #[test]
    fn unsafe_components_reject_traversal_and_separators() {
        assert!(!is_safe_component(""));
        assert!(!is_safe_component("/abs/path"));
        assert!(!is_safe_component("../escape"));
        assert!(!is_safe_component("a/../b"));
        assert!(!is_safe_component("..\\win"));
        assert!(!is_safe_component("sub/dir"));
        assert!(!is_safe_component(".."));
    }

    #[tokio::test]
    async fn handle_message_writes_input_file_for_valid_message() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let cas = CasStore::new(tmp.path().join("cas")).await.expect("cas");
        let cid = cas.put(b"prompt body").await.expect("put");
        let msg = BrokerMessage::new("wf-1", "00_spec", "01_next", cid);

        handle_message(&cas, tmp.path(), &msg)
            .await
            .expect("handled");

        let written = tmp
            .path()
            .join(".zerochain")
            .join("workflows")
            .join("wf-1")
            .join("01_next")
            .join("input")
            .join("00_spec.md");
        assert_eq!(std::fs::read(&written).expect("read"), b"prompt body");
    }

    #[tokio::test]
    async fn handle_message_rejects_escaping_to_stage() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let cas = CasStore::new(tmp.path().join("cas")).await.expect("cas");
        let cid = cas.put(b"prompt body").await.expect("put");

        for to_stage in ["../../evil", "/abs/evil", "sub/dir", "..\\win"] {
            let msg = BrokerMessage::new("wf-1", "00_spec", to_stage, cid.clone());
            assert!(
                handle_message(&cas, tmp.path(), &msg).await.is_err(),
                "to_stage {to_stage} must be rejected"
            );
        }
        // Nothing may have been created outside the workflow tree.
        assert!(!tmp.path().join("evil").exists());
        assert!(!tmp.path().join(".zerochain").join("evil").exists());
    }

    #[tokio::test]
    async fn handle_message_rejects_escaping_workflow_id_and_from_stage() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let cas = CasStore::new(tmp.path().join("cas")).await.expect("cas");
        let cid = cas.put(b"prompt body").await.expect("put");

        let bad_id = BrokerMessage::new("../wf", "00_spec", "01_next", cid.clone());
        assert!(handle_message(&cas, tmp.path(), &bad_id).await.is_err());

        let bad_from = BrokerMessage::new("wf-1", "../00_spec", "01_next", cid);
        assert!(handle_message(&cas, tmp.path(), &bad_from).await.is_err());

        let outside_wf = tmp.path().parent().expect("parent").join("00_spec.md");
        assert!(!outside_wf.exists());
    }
}
