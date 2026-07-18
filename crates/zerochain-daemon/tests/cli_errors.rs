//! CLI error-message tests for `zerochain init`.

use std::process::Command;

fn zerochain_bin() -> &'static str {
    env!("CARGO_BIN_EXE_zerochain")
}

fn run_zerochain(workspace: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(zerochain_bin())
        .arg("--workspace")
        .arg(workspace)
        .args(args)
        .output()
        .expect("failed to spawn zerochain")
}

#[test]
fn duplicate_init_without_force_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let first = run_zerochain(tmp.path(), &["init", "--name", "foo"]);
    assert!(
        first.status.success(),
        "first init should succeed: stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let second = run_zerochain(tmp.path(), &["init", "--name", "foo"]);
    assert!(
        !second.status.success(),
        "second init without --force should fail"
    );

    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("workflow \"foo\" already exists; use --force to reset"),
        "expected friendly duplicate-init error, got: {stderr}"
    );
    assert!(
        !stderr.contains("os error 17"),
        "error should not be a raw os error: {stderr}"
    );
}

#[test]
fn duplicate_init_with_force_succeeds() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let first = run_zerochain(tmp.path(), &["init", "--name", "foo"]);
    assert!(
        first.status.success(),
        "first init should succeed: stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let second = run_zerochain(tmp.path(), &["init", "--name", "foo", "--force"]);
    assert!(
        second.status.success(),
        "second init with --force should succeed: stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout.contains("initialized workflow: foo"),
        "got: {stdout}"
    );
}

#[test]
fn workspace_env_and_flag_conflict_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let other = tmp.path().join("other");
    let default = tmp.path().join("default");

    let output = Command::new(zerochain_bin())
        .arg("--workspace")
        .arg(&default)
        .env("ZEROCHAIN_WORKSPACE", &other)
        .args(["init", "--name", "conflict"])
        .output()
        .expect("failed to spawn zerochain");

    assert!(
        !output.status.success(),
        "conflicting workspace sources should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("workspace conflict"),
        "expected workspace conflict error, got: {stderr}"
    );
}
