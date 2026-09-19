use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use zerochain_error::{Result, ZerochainError};

use crate::tool::Tool;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 120_000;

const ALLOWED_COMMANDS: &[&str] = &[
    "cat", "ls", "echo", "grep", "find", "jj", "git", "cargo", "rustc", "python3", "python", "node",
];

fn parse_tokens(cmd: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    // Tracks whether the current token has begun, so that empty quoted
    // arguments (`''` or `""`) are preserved instead of dropped.
    let mut token_started = false;

    for c in cmd.chars() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                token_started = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                token_started = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if token_started {
                    tokens.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            c => {
                current.push(c);
                token_started = true;
            }
        }
    }
    if token_started {
        tokens.push(current);
    }
    tokens
}

fn has_forbidden_metacharacters(cmd: &str) -> bool {
    let forbidden = [';', '|', '&', '$', '`', '>', '<'];
    let mut in_single = false;
    let mut in_double = false;

    for c in cmd.chars() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            _ if !in_single && !in_double && forbidden.contains(&c) => return true,
            _ => {}
        }
    }
    false
}

fn validate_command(cmd: &str) -> Result<Vec<String>> {
    let tokens = parse_tokens(cmd);
    let program = tokens.first().ok_or_else(|| ZerochainError::InvalidInput {
        message: "empty command".to_string(),
    })?;

    if !ALLOWED_COMMANDS.contains(&program.as_str()) {
        return Err(ZerochainError::InvalidInput {
            message: format!("command not allowed: {program}"),
        });
    }
    if has_forbidden_metacharacters(cmd) {
        return Err(ZerochainError::InvalidInput {
            message: "command contains forbidden shell metacharacters".to_string(),
        });
    }
    Ok(tokens)
}

/// Execute a sandboxed shell command from an allow-list with a timeout.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Run a sandboxed shell command from an allowed list with a timeout."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Command to run." },
                "timeout_ms": { "type": "number", "description": "Timeout in milliseconds (default 30000, max 120000)." },
                "workspace_root": {
                    "type": "string",
                    "description": "Injected by the engine; do not set manually."
                }
            },
            "required": ["command"]
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ZerochainError::InvalidInput {
                message: "missing 'command' field".to_string(),
            })?;

        // A timeout of 0 would fire immediately; treat it as "use the default".
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .map(|t| {
                if t == 0 {
                    DEFAULT_TIMEOUT_MS
                } else {
                    t.min(MAX_TIMEOUT_MS)
                }
            })
            .unwrap_or(DEFAULT_TIMEOUT_MS);

        let workspace_root = input
            .get("workspace_root")
            .and_then(Value::as_str)
            .unwrap_or(".");
        let root = Path::new(workspace_root)
            .canonicalize()
            .map_err(|e| ZerochainError::Io {
                path: PathBuf::from(workspace_root),
                source: e,
            })?;
        if !root.is_dir() {
            return Err(ZerochainError::InvalidInput {
                message: "workspace_root is not a directory".to_string(),
            });
        }

        let tokens = validate_command(command)?;
        let program = &tokens[0];
        let args: Vec<&str> = tokens.iter().skip(1).map(|s| s.as_str()).collect();

        let timeout = Duration::from_millis(timeout_ms);
        let mut child = Command::new(program)
            .args(&args)
            .current_dir(&root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| ZerochainError::Io {
                path: Path::new(program).to_path_buf(),
                source: e,
            })?;

        let mut stdout = child.stdout.take().expect("stdout was piped");
        let mut stderr = child.stderr.take().expect("stderr was piped");

        // Read raw bytes: command output may not be valid UTF-8 (binary
        // artifacts, locale noise) and must not fail the whole call.
        let mut stdout_buf: Vec<u8> = Vec::new();
        let mut stderr_buf: Vec<u8> = Vec::new();

        let stdout_fut = stdout.read_to_end(&mut stdout_buf);
        let stderr_fut = stderr.read_to_end(&mut stderr_buf);

        let result = tokio::time::timeout(timeout, async {
            let (_, _, status) = tokio::try_join!(stdout_fut, stderr_fut, child.wait())?;
            Ok::<_, std::io::Error>(status)
        })
        .await;

        match result {
            Ok(Ok(status)) => Ok(json!({
                "stdout": String::from_utf8_lossy(&stdout_buf),
                "stderr": String::from_utf8_lossy(&stderr_buf),
                "exit_code": status.code().unwrap_or(-1)
            })),
            Ok(Err(e)) => Err(ZerochainError::Io {
                path: Path::new(program).to_path_buf(),
                source: e,
            }),
            Err(_) => {
                let _ = child.start_kill();
                Err(ZerochainError::Other {
                    message: format!("command timed out after {timeout_ms}ms"),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_tokens_preserves_empty_quoted_arguments() {
        assert_eq!(
            parse_tokens("python3 -c ''"),
            vec!["python3".to_string(), "-c".to_string(), String::new()]
        );
        assert_eq!(
            parse_tokens("echo '' \"\""),
            vec!["echo".to_string(), String::new(), String::new()]
        );
        // A bare quoted empty string is a single (empty) argument.
        assert_eq!(parse_tokens("''"), vec![String::new()]);
    }

    #[test]
    fn parse_tokens_unquoted_behaviour_unchanged() {
        assert_eq!(
            parse_tokens("echo hello world"),
            vec!["echo".to_string(), "hello".to_string(), "world".to_string()]
        );
        assert!(parse_tokens("").is_empty());
        assert!(parse_tokens("   ").is_empty());
        assert_eq!(
            parse_tokens("echo 'a b' c"),
            vec!["echo".to_string(), "a b".to_string(), "c".to_string()]
        );
    }

    #[tokio::test]
    async fn non_utf8_output_is_lossy_not_an_error() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(
            workspace.path().join("blob.bin"),
            [0xffu8, 0xfe, b'a', 0x80],
        )
        .unwrap();

        let tool = ShellTool;
        let result = tool
            .run(json!({
                "command": "cat blob.bin",
                "workspace_root": workspace.path().to_str().unwrap(),
            }))
            .await
            .expect("non-UTF-8 output must not fail the call");

        assert_eq!(result.get("exit_code").unwrap().as_i64().unwrap(), 0);
        let stdout = result.get("stdout").unwrap().as_str().unwrap();
        assert!(
            stdout.contains('\u{FFFD}'),
            "expected lossy replacement: {stdout:?}"
        );
        assert!(stdout.contains('a'));
    }

    #[tokio::test]
    async fn zero_timeout_uses_default() {
        let workspace = tempfile::tempdir().unwrap();

        let tool = ShellTool;
        let result = tool
            .run(json!({
                "command": "echo hi",
                "timeout_ms": 0,
                "workspace_root": workspace.path().to_str().unwrap(),
            }))
            .await
            .expect("timeout_ms of 0 must fall back to the default");
        assert_eq!(result.get("exit_code").unwrap().as_i64().unwrap(), 0);
        assert_eq!(result.get("stdout").unwrap().as_str().unwrap().trim(), "hi");
    }
}
