use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
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
                "timeout_ms": { "type": "number", "description": "Timeout in milliseconds (default 30000, max 120000)." }
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
            .map(Path::new)
            .unwrap_or(Path::new("."));

        let tokens = validate_command(command)?;
        let program = &tokens[0];
        let args: Vec<&str> = tokens.iter().skip(1).map(|s| s.as_str()).collect();

        let timeout = Duration::from_millis(timeout_ms);
        let output = tokio::time::timeout(
            timeout,
            Command::new(program)
                .args(&args)
                .current_dir(workspace_root)
                .output(),
        )
        .await
        .map_err(|_| ZerochainError::Other {
            message: format!("command timed out after {timeout_ms}ms"),
        })?
        .map_err(|e| ZerochainError::Io {
            path: Path::new(program).to_path_buf(),
            source: e,
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok(json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": output.status.code().unwrap_or(-1)
        }))
    }
}
