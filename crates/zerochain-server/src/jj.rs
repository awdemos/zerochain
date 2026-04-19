use std::path::Path;
use std::process::Command;

use tracing::{debug, warn};

pub fn init_repo(workspace: &Path) -> bool {
    let jj_dir = workspace.join(".jj");
    if jj_dir.exists() {
        debug!("jj repo already initialized");
        return true;
    }

    let result = Command::new("jj")
        .args(["init", "--git"])
        .current_dir(workspace)
        .output();

    match result {
        Ok(output) if output.status.success() => {
            let _ = Command::new("jj")
                .args(["config", "set", "user.name", "zerochain"])
                .current_dir(workspace)
                .output();
            let _ = Command::new("jj")
                .args(["config", "set", "user.email", "zerochain@daemon"])
                .current_dir(workspace)
                .output();
            debug!("jj repo initialized");
            true
        }
        Ok(output) => {
            warn!(
                stderr = %String::from_utf8_lossy(&output.stderr),
                "jj init failed"
            );
            false
        }
        Err(e) => {
            debug!("jj not available: {e}");
            false
        }
    }
}

pub fn auto_commit(workspace: &Path, message: &str) {
    let result = Command::new("jj")
        .args(["commit", "-m", message])
        .current_dir(workspace)
        .output();

    match result {
        Ok(output) if output.status.success() => {
            debug!(message, "jj auto-commit");
        }
        Ok(output) => {
            warn!(
                stderr = %String::from_utf8_lossy(&output.stderr),
                message,
                "jj commit failed"
            );
        }
        Err(e) => {
            debug!("jj not available for commit: {e}");
        }
    }
}

pub fn commit_stage_complete(workspace: &Path, workflow_id: &str, stage_raw: &str) {
    auto_commit(workspace, &format!("stage {stage_raw} complete: {workflow_id}"));
}

pub fn commit_stage_error(workspace: &Path, workflow_id: &str, stage_raw: &str) {
    auto_commit(workspace, &format!("stage {stage_raw} error: {workflow_id}"));
}

pub fn commit_dag_mutation(workspace: &Path, action: &str, stage_name: &str) {
    auto_commit(workspace, &format!("dag: {action} {stage_name}"));
}

pub fn commit_state_change(workspace: &Path, key: &str) {
    auto_commit(workspace, &format!("state: {key}"));
}

#[cfg(test)]
mod tests {

    #[test]
    fn commit_message_format() {
        let msg = format!("stage {} complete: {}", "00_spec", "my-workflow");
        assert_eq!(msg, "stage 00_spec complete: my-workflow");
    }

    #[test]
    fn dag_mutation_message() {
        let msg = format!("dag: {} {}", "inserted", "01b_review");
        assert_eq!(msg, "dag: inserted 01b_review");
    }
}
