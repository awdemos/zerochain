use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use zerochain_error::ZerochainError;
use zerochain_tools::{ShellTool, Tool};

fn temp_workspace() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zerochain-shell-tool-test-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

async fn python3_available() -> bool {
    tokio::process::Command::new("python3")
        .arg("--version")
        .output()
        .await
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn shell_tool_runs_allowed_command() {
    let workspace = temp_workspace();
    let file = workspace.join("hello.txt");
    tokio::fs::write(&file, "hello shell").await.unwrap();

    let tool = ShellTool;
    let input = serde_json::json!({
        "workspace_root": workspace.to_str().unwrap(),
        "command": "cat hello.txt"
    });
    let result = tool.run(input).await.unwrap();

    assert_eq!(
        result.get("stdout").unwrap().as_str().unwrap(),
        "hello shell"
    );
    assert_eq!(result.get("stderr").unwrap().as_str().unwrap(), "");
    assert_eq!(result.get("exit_code").unwrap().as_i64().unwrap(), 0);

    tokio::fs::remove_dir_all(&workspace).await.ok();
}

#[tokio::test]
async fn shell_tool_rejects_disallowed_command() {
    let workspace = temp_workspace();

    let tool = ShellTool;
    let input = serde_json::json!({
        "workspace_root": workspace.to_str().unwrap(),
        "command": "rm -rf /"
    });
    let err = tool.run(input).await.unwrap_err();
    assert!(matches!(err, ZerochainError::InvalidInput { .. }));

    tokio::fs::remove_dir_all(&workspace).await.ok();
}

#[tokio::test]
async fn shell_tool_rejects_metacharacters() {
    let workspace = temp_workspace();

    let tool = ShellTool;
    let input = serde_json::json!({
        "workspace_root": workspace.to_str().unwrap(),
        "command": "cat hello.txt; rm -rf /"
    });
    let err = tool.run(input).await.unwrap_err();
    assert!(matches!(err, ZerochainError::InvalidInput { .. }));

    tokio::fs::remove_dir_all(&workspace).await.ok();
}

#[tokio::test]
async fn shell_tool_clamps_timeout() {
    let workspace = temp_workspace();

    let tool = ShellTool;
    let input = serde_json::json!({
        "workspace_root": workspace.to_str().unwrap(),
        "command": "echo hi",
        "timeout_ms": 999_999
    });
    let result = tool.run(input).await.unwrap();

    assert_eq!(result.get("stdout").unwrap().as_str().unwrap().trim(), "hi");
    assert_eq!(result.get("exit_code").unwrap().as_i64().unwrap(), 0);

    tokio::fs::remove_dir_all(&workspace).await.ok();
}

#[tokio::test]
async fn shell_tool_respects_short_timeout() {
    if !python3_available().await {
        return;
    }

    let workspace = temp_workspace();

    let tool = ShellTool;
    let input = serde_json::json!({
        "workspace_root": workspace.to_str().unwrap(),
        "command": "python3 -c \"import time; time.sleep(10)\"",
        "timeout_ms": 100
    });
    let err = tool.run(input).await.unwrap_err();
    assert!(matches!(err, ZerochainError::Other { .. }));

    tokio::fs::remove_dir_all(&workspace).await.ok();
}
