use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use zerochain_error::ZerochainError;
use zerochain_tools::{ReadFileTool, Tool, WriteFileTool};

fn temp_workspace() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zerochain-fs-tool-test-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

#[tokio::test]
async fn read_file_reads_existing_file() {
    let workspace = temp_workspace();
    let file = workspace.join("test.txt");
    tokio::fs::write(&file, "hello world").await.unwrap();

    let tool = ReadFileTool;
    let input = serde_json::json!({
        "workspace_root": workspace.to_str().unwrap(),
        "path": "test.txt"
    });
    let result = tool.run(input).await.unwrap();

    assert_eq!(
        result.get("content").unwrap().as_str().unwrap(),
        "hello world"
    );
    assert!(result.get("exists").unwrap().as_bool().unwrap());

    tokio::fs::remove_dir_all(&workspace).await.ok();
}

#[tokio::test]
async fn read_file_reports_missing_file() {
    let workspace = temp_workspace();

    let tool = ReadFileTool;
    let input = serde_json::json!({
        "workspace_root": workspace.to_str().unwrap(),
        "path": "missing.txt"
    });
    let result = tool.run(input).await.unwrap();

    assert_eq!(result.get("content").unwrap().as_str().unwrap(), "");
    assert!(!result.get("exists").unwrap().as_bool().unwrap());

    tokio::fs::remove_dir_all(&workspace).await.ok();
}

#[tokio::test]
async fn write_file_creates_file_and_parents() {
    let workspace = temp_workspace();

    let tool = WriteFileTool;
    let input = serde_json::json!({
        "workspace_root": workspace.to_str().unwrap(),
        "path": "nested/dir/file.txt",
        "content": "hello"
    });
    let result = tool.run(input).await.unwrap();

    assert!(result.get("written").unwrap().as_bool().unwrap());
    assert_eq!(result.get("bytes").unwrap().as_u64().unwrap(), 5);

    let file = workspace.join("nested/dir/file.txt");
    let content = tokio::fs::read_to_string(&file).await.unwrap();
    assert_eq!(content, "hello");

    tokio::fs::remove_dir_all(&workspace).await.ok();
}

#[tokio::test]
async fn write_file_rejects_path_escape() {
    let workspace = temp_workspace();

    let tool = WriteFileTool;
    let input = serde_json::json!({
        "workspace_root": workspace.to_str().unwrap(),
        "path": "../outside.txt",
        "content": "escaped"
    });
    let err = tool.run(input).await.unwrap_err();
    assert!(matches!(err, ZerochainError::InvalidInput { .. }));

    tokio::fs::remove_dir_all(&workspace).await.ok();
}
