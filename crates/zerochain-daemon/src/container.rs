use std::path::{Path, PathBuf};

use crate::error::DaemonError;

#[non_exhaustive]
pub struct ContainerConfig {
    pub image: String,
    pub stage_dir: PathBuf,
    pub output_dir: PathBuf,
    pub env_vars: Vec<(String, String)>,
    pub command: Vec<String>,
    pub workspace_root: PathBuf,
}

#[non_exhaustive]
pub struct ContainerResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub struct ContainerExecutor {
    runtime: ContainerRuntime,
}

enum ContainerRuntime {
    Docker,
    Podman,
}

impl ContainerExecutor {
    pub fn detect() -> Option<Self> {
        if which_exists("docker") {
            tracing::info!("container executor: docker");
            Some(Self {
                runtime: ContainerRuntime::Docker,
            })
        } else if which_exists("podman") {
            tracing::info!("container executor: podman");
            Some(Self {
                runtime: ContainerRuntime::Podman,
            })
        } else {
            None
        }
    }

    pub fn runtime_name(&self) -> &str {
        match self.runtime {
            ContainerRuntime::Docker => "docker",
            ContainerRuntime::Podman => "podman",
        }
    }

    pub async fn run_stage(&self, config: &ContainerConfig) -> Result<ContainerResult, DaemonError> {
        let runtime = self.runtime_name();
        let container_name = format!(
            "zerochain-stage-{}",
            config
                .stage_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".into())
        );

        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            container_name.clone(),
        ];

        args.push("-v".to_string());
        args.push(format!(
            "{}:/stage:rw",
            config.stage_dir.display()
        ));

        args.push("-v".to_string());
        args.push(format!(
            "{}:/output:rw",
            config.output_dir.display()
        ));

        args.push("-v".to_string());
        args.push(format!(
            "{}:/workspace:rw",
            config.workspace_root.display()
        ));

        for (key, value) in &config.env_vars {
            args.push("-e".to_string());
            args.push(format!("{key}={value}"));
        }

        args.push(config.image.clone());
        args.extend(config.command.iter().cloned());

        tracing::info!(
            runtime = runtime,
            container = %container_name,
            image = %config.image,
            "spawning container for stage execution"
        );

        let output = tokio::process::Command::new(runtime)
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| DaemonError::ContainerSpawn(format!("failed to spawn container: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        if !output.status.success() {
            tracing::error!(
                container = %container_name,
                exit_code,
                stderr = %stderr,
                "container execution failed"
            );
            return Err(DaemonError::ContainerExec(format!(
                "container exited with code {exit_code}: {stderr}"
            )));
        }

        tracing::info!(
            container = %container_name,
            exit_code,
            stdout_bytes = stdout.len(),
            "container execution complete"
        );

        Ok(ContainerResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    pub async fn pull_image(&self, image: &str) -> Result<(), DaemonError> {
        let runtime = self.runtime_name();
        tracing::info!(runtime = runtime, image = %image, "pulling container image");

        let output = tokio::process::Command::new(runtime)
            .args(["pull", image])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| DaemonError::ContainerSpawn(format!("failed to pull image: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DaemonError::ContainerSpawn(format!(
                "failed to pull image {image}: {stderr}"
            )));
        }

        Ok(())
    }

    pub async fn build_stage_image(
        &self,
        context_dir: &Path,
        dockerfile: &str,
        tag: &str,
    ) -> Result<(), DaemonError> {
        let runtime = self.runtime_name();
        let dockerfile_path = context_dir.join("Dockerfile.stage");

        tokio::fs::write(&dockerfile_path, dockerfile)
            .await
            .map_err(|e| DaemonError::io(&dockerfile_path, e))?;

        tracing::info!(
            runtime = runtime,
            context = %context_dir.display(),
            tag = %tag,
            "building stage container image"
        );

        let output = tokio::process::Command::new(runtime)
            .args([
                "build",
                "-f",
                &dockerfile_path.to_string_lossy(),
                "-t",
                tag,
                &context_dir.to_string_lossy(),
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| DaemonError::ContainerSpawn(format!("failed to build image: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DaemonError::ContainerSpawn(format!(
                "failed to build image {tag}: {stderr}"
            )));
        }

        Ok(())
    }
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn default_stage_image() -> String {
    std::env::var("ZEROCHAIN_STAGE_IMAGE")
        .unwrap_or_else(|_| "cgr.dev/chainguard/wolfi-base:latest".into())
}

pub fn generate_stage_dockerfile(base_image: &str) -> String {
    format!(
        r#"FROM {base_image}
RUN apk add --no-cache ca-certificates curl
COPY --from=zerochain-builder /usr/local/bin/zerochain /usr/local/bin/zerochain
ENTRYPOINT ["zerochain"]
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_stage_image_is_chainguard() {
        let image = default_stage_image();
        assert!(image.contains("chainguard") || image.contains("wolfi"));
    }

    #[test]
    fn generate_dockerfile_contains_from() {
        let df = generate_stage_dockerfile("test:latest");
        assert!(df.contains("FROM test:latest"));
        assert!(df.contains("ENTRYPOINT"));
    }

    #[test]
    fn detect_returns_none_without_runtime() {
        if !which_exists("docker") && !which_exists("podman") {
            assert!(ContainerExecutor::detect().is_none());
        }
    }

    #[test]
    fn container_config_stores_paths() {
        let config = ContainerConfig {
            image: "test:latest".into(),
            stage_dir: PathBuf::from("/tmp/stage"),
            output_dir: PathBuf::from("/tmp/output"),
            env_vars: vec![("KEY".into(), "VALUE".into())],
            command: vec!["echo".into(), "hello".into()],
            workspace_root: PathBuf::from("/tmp/workspace"),
        };
        assert_eq!(config.image, "test:latest");
        assert_eq!(config.env_vars.len(), 1);
    }
}
