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

    for c in cmd.chars() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
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

        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .map(|t| t.min(MAX_TIMEOUT_MS))
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

        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();

        let stdout_fut = stdout.read_to_string(&mut stdout_buf);
        let stderr_fut = stderr.read_to_string(&mut stderr_buf);

        let result = tokio::time::timeout(timeout, async {
            let (_, _, status) = tokio::try_join!(stdout_fut, stderr_fut, child.wait())?;
            Ok::<_, std::io::Error>(status)
        })
        .await;

        match result {
            Ok(Ok(status)) => Ok(json!({
                "stdout": stdout_buf,
                "stderr": stderr_buf,
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
