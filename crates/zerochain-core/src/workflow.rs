use std::path::{Path, PathBuf};

use crate::error::{io_err, Error, Result};
use crate::plan::ExecutionPlan;
use crate::stage::{Stage, StageId};
use crate::task::Task;

#[must_use] pub fn is_valid_workflow_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Workflow {
    pub root: PathBuf,
    pub id: String,
    pub stages: Vec<Stage>,
    pub task: Option<Task>,
}

impl Workflow {
    /// Load a workflow from a directory on disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory does not exist, contains invalid stage definitions,
    /// or is missing required files (e.g. `CONTEXT.md`).
    pub async fn from_dir(path: &Path) -> Result<Self> {
        let metadata = tokio::fs::metadata(path).await.map_err(|e| io_err(path.to_path_buf(), e))?;
        if !metadata.is_dir() {
            return Err(Error::WorkflowNotFound {
                path: path.to_path_buf(),
            });
        }

        let id = path
            .file_name().map_or_else(|| "unknown".to_string(), |n| n.to_string_lossy().to_string());

        let task = Self::find_task(path).await?;

        let mut stages = Vec::new();
        let mut entries = tokio::fs::read_dir(path).await.map_err(|e| io_err(path.to_path_buf(), e))?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| io_err(path.to_path_buf(), e))? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if StageId::parse(&name).is_err() {
                continue;
            }
            let stage_path = entry.path();
            let stage_meta = entry.metadata().await.map_err(|e| io_err(stage_path.clone(), e))?;
            if !stage_meta.is_dir() {
                continue;
            }
            stages.push(Stage::from_dir(&stage_path).await?);
        }

        if stages.is_empty() {
            return Err(Error::NoStages {
                path: path.to_path_buf(),
            });
        }

        stages.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(Workflow {
            root: path.to_path_buf(),
            id,
            stages,
            task,
        })
    }

    #[must_use] pub fn execution_plan(&self) -> ExecutionPlan {
        ExecutionPlan::from_stages(&self.stages)
    }

    pub async fn init(task: &Task, base_path: &Path) -> Result<Self> {
        let sanitized_id = task
            .id
            .replace(['/', '\\'], "-")
            .replace("..", "-")
            .replace('\0', "");
        if sanitized_id.is_empty() || sanitized_id.len() > 128 {
            return Err(Error::InvalidWorkflowName {
                name: task.id.clone(),
            });
        }
        let workflow_dir = base_path.join(&sanitized_id);
        tokio::fs::create_dir_all(&workflow_dir)
            .await
            .map_err(|e| io_err(workflow_dir.clone(), e))?;

        let stage_names = task.stage_names();
        let stage_defs: Vec<String> = if stage_names.is_empty() {
            (0..3).map(|i| format!("{i:02}_stage_{i}")).collect()
        } else {
            stage_names
        };

        let mut prev_output: Option<PathBuf> = None;

        for stage_name in &stage_defs {
            let stage_dir = workflow_dir.join(stage_name);
            tokio::fs::create_dir_all(&stage_dir)
                .await
                .map_err(|e| io_err(stage_dir.clone(), e))?;

            let output_dir = stage_dir.join("output");
            tokio::fs::create_dir_all(&output_dir)
                .await
                .map_err(|e| io_err(output_dir.clone(), e))?;

            let input_dir = stage_dir.join("input");
            if let Some(ref prev) = prev_output {
                create_stage_input_link(prev, &input_dir).await?;
            } else {
                tokio::fs::create_dir_all(&input_dir)
                    .await
                    .map_err(|e| io_err(input_dir.clone(), e))?;
            }

            let ctx_content = format!("---\nrole: {stage_name}\n---\n# {stage_name}\n");
            let ctx_path = stage_dir.join("CONTEXT.md");
            tokio::fs::write(&ctx_path, ctx_content)
                .await
                .map_err(|e| io_err(ctx_path, e))?;

            prev_output = Some(output_dir);
        }

        Workflow::from_dir(&workflow_dir).await
    }

    #[must_use] pub fn stage_by_id(&self, id: &StageId) -> Option<&Stage> {
        self.stages.iter().find(|s| s.id == *id)
    }

    #[must_use] pub fn stage_by_name(&self, name: &str) -> Option<&Stage> {
        self.stages.iter().find(|s| s.id.name == name)
    }

    #[must_use] pub fn stage_index(&self, raw: &str) -> Option<usize> {
        self.stages.iter().position(|s| s.id.raw == raw)
    }

    pub async fn insert_stage_after(
        &mut self,
        after_raw: &str,
        new_stage_name: &str,
    ) -> Result<()> {
        let idx = self
            .stage_index(after_raw)
            .ok_or_else(|| Error::InvalidStageName {
                name: after_raw.to_string(),
            })?;

        let after_stage = &self.stages[idx];
        let next_seq = after_stage.id.sequence;

        let new_seq = next_seq + 1;
        let new_raw = format!("{new_seq:02}_{new_stage_name}");

        let new_dir = self.root.join(&new_raw);
        tokio::fs::create_dir_all(new_dir.join("input"))
            .await
            .map_err(|e| io_err(new_dir.clone(), e))?;
        tokio::fs::create_dir_all(new_dir.join("output"))
            .await
            .map_err(|e| io_err(new_dir.clone(), e))?;
        tokio::fs::write(
            new_dir.join("CONTEXT.md"),
            format!("---\nrole: {new_stage_name}\n---\n# {new_stage_name}\n"),
        )
        .await
        .map_err(|e| io_err(new_dir.join("CONTEXT.md"), e))?;

        let new_stage = Stage::from_dir(&new_dir).await?;
        self.stages.insert(idx + 1, new_stage);

        Ok(())
    }

    pub async fn remove_stage(&mut self, raw: &str) -> Result<()> {
        let idx = self
            .stage_index(raw)
            .ok_or_else(|| Error::InvalidStageName {
                name: raw.to_string(),
            })?;
        let stage_dir = self.stages[idx].path.clone();
        tokio::fs::remove_dir_all(&stage_dir)
            .await
            .map_err(|e| io_err(stage_dir, e))?;
        self.stages.remove(idx);
        Ok(())
    }

    async fn find_task(path: &Path) -> Result<Option<Task>> {
        let candidates = ["task.md", "TASK.md", "Backlog.md", "backlog.md"];
        for candidate in candidates {
            let task_path = path.join(candidate);
            let exists = tokio::fs::try_exists(&task_path)
                .await
                .map_err(|e| crate::error::io_err(&task_path, e))?;
            if exists {
                match Task::from_file(&task_path).await {
                    Ok(task) => return Ok(Some(task)),
                    Err(e) => {
                        tracing::warn!(path = %task_path.display(), error = %e, "failed to parse task file");
                    }
                }
            }
        }

        let mut entries = tokio::fs::read_dir(path).await.map_err(|e| crate::error::io_err(path, e))?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| crate::error::io_err(path, e))? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("00_spec") {
                let task_path = entry.path().join("task.md");
                let exists = tokio::fs::try_exists(&task_path)
                    .await
                    .map_err(|e| crate::error::io_err(&task_path, e))?;
                if exists {
                    match Task::from_file(&task_path).await {
                        Ok(task) => return Ok(Some(task)),
                        Err(e) => {
                            tracing::warn!(path = %task_path.display(), error = %e, "failed to parse task file in 00_spec");
                        }
                    }
                }
            }
        }

        Ok(None)
    }
}

/// Creates a link from `input_dir` to `prev_output` so that a stage's input
/// directory inherits the previous stage's output.
///
/// - On Unix: uses a symbolic link.
/// - On Windows: tries a junction point first (no admin rights needed),
///   falling back to a recursive directory copy if junction creation fails.
async fn create_stage_input_link(prev_output: &Path, input_dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        tokio::fs::symlink(prev_output, input_dir)
            .await
            .map_err(|e| io_err(input_dir.to_path_buf(), e))?;
        tracing::debug!(
            source = %prev_output.display(),
            target = %input_dir.display(),
            method = "symlink",
            "created stage input link"
        );
    }

    #[cfg(windows)]
    {
        let src = prev_output.to_path_buf();
        let dst = input_dir.to_path_buf();
        let src_display = src.display().to_string();
        let dst_display = dst.display().to_string();

        let junction_result = tokio::task::spawn_blocking(move || {
            junction::create(&src, &dst)
        }).await;

        match junction_result {
            Ok(Ok(())) => {
                tracing::debug!(
                    source = %src_display,
                    target = %dst_display,
                    method = "junction",
                    "created stage input link"
                );
            }
            Ok(Err(e)) => {
                tracing::debug!(
                    error = %e,
                    "junction creation failed, falling back to recursive copy"
                );
                copy_dir_recursive(prev_output, input_dir).await?;
                tracing::debug!(
                    source = %src_display,
                    target = %dst_display,
                    method = "copy",
                    "created stage input link"
                );
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "junction task panicked, falling back to recursive copy"
                );
                copy_dir_recursive(prev_output, input_dir).await?;
                tracing::debug!(
                    source = %src_display,
                    target = %dst_display,
                    method = "copy",
                    "created stage input link"
                );
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
async fn copy_dir_recursive(source: &Path, target: &Path) -> Result<()> {
    tokio::fs::create_dir_all(target)
        .await
        .map_err(|e| io_err(target.to_path_buf(), e))?;

    let mut entries = tokio::fs::read_dir(source)
        .await
        .map_err(|e| io_err(source.to_path_buf(), e))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| io_err(source.to_path_buf(), e))?
    {
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dst_path = target.join(&file_name);

        let file_type = entry
            .file_type()
            .await
            .map_err(|e| io_err(&src_path, e))?;

        if file_type.is_dir() {
            Box::pin(copy_dir_recursive(&src_path, &dst_path)).await?;
        } else if file_type.is_file() {
            tokio::fs::copy(&src_path, &dst_path)
                .await
                .map_err(|e| io_err(&src_path, e))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_workflow(dir: &Path) -> PathBuf {
        let workflow_dir = dir.join("test-workflow");
        let stage_dirs = ["00_spec", "01_analyze", "02_implement"];

        for name in &stage_dirs {
            let stage_dir = workflow_dir.join(name);
            tokio::fs::create_dir_all(stage_dir.join("input"))
                .await
                .unwrap();
            tokio::fs::create_dir_all(stage_dir.join("output"))
                .await
                .unwrap();
            tokio::fs::write(
                stage_dir.join("CONTEXT.md"),
                format!("---\nrole: {name}\n---\n# {name}\n"),
            )
            .await
            .unwrap();
        }

        workflow_dir
    }

    #[tokio::test]
    async fn from_dir_parses_stages() {
        let tmp = tempfile::tempdir().unwrap();
        let workflow_dir = create_test_workflow(tmp.path()).await;

        let wf = Workflow::from_dir(&workflow_dir).await.unwrap();
        assert_eq!(wf.stages.len(), 3);
        assert_eq!(wf.stages[0].id.raw, "00_spec");
        assert_eq!(wf.stages[1].id.raw, "01_analyze");
        assert_eq!(wf.stages[2].id.raw, "02_implement");
    }

    #[tokio::test]
    async fn from_dir_reads_stage_state() {
        let tmp = tempfile::tempdir().unwrap();
        let workflow_dir = create_test_workflow(tmp.path()).await;

        tokio::fs::write(workflow_dir.join("01_analyze").join(".complete"), "")
            .await
            .unwrap();
        tokio::fs::write(workflow_dir.join("02_implement").join(".error"), "fail")
            .await
            .unwrap();

        let wf = Workflow::from_dir(&workflow_dir).await.unwrap();
        assert!(!wf.stages[0].is_complete);
        assert!(wf.stages[1].is_complete);
        assert!(!wf.stages[2].is_complete);
        assert!(wf.stages[2].is_error);
    }

    #[tokio::test]
    async fn from_dir_rejects_missing() {
        let result = Workflow::from_dir(Path::new("/nonexistent/path")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn init_creates_workflow_from_task() {
        let tmp = tempfile::tempdir().unwrap();
        let task = Task {
            id: "TASK-100".to_string(),
            title: "Test init".to_string(),
            status: "todo".to_string(),
            priority: Some("high".to_string()),
            execution: Some(crate::task::TaskExecution {
                stages: vec![
                    "00_spec".to_string(),
                    "01_build".to_string(),
                    "02_test".to_string(),
                ],
                strategy: Some("sequential".to_string()),
            }),
            acceptance_criteria: vec![],
            description: "Init test".to_string(),
            source_path: None,
        };

        let wf = Workflow::init(&task, tmp.path()).await.unwrap();
        assert_eq!(wf.id, "TASK-100");
        assert_eq!(wf.stages.len(), 3);
        assert_eq!(wf.stages[0].id.raw, "00_spec");
        assert_eq!(wf.stages[1].id.raw, "01_build");
        assert_eq!(wf.stages[2].id.raw, "02_test");
    }

    #[tokio::test]
    async fn stage_lookup_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let workflow_dir = create_test_workflow(tmp.path()).await;
        let wf = Workflow::from_dir(&workflow_dir).await.unwrap();

        let stage = wf.stage_by_name("analyze").unwrap();
        assert_eq!(stage.id.raw, "01_analyze");
        assert!(wf.stage_by_name("nonexistent").is_none());
    }

    #[tokio::test]
    async fn init_links_stage_input_to_previous_output() {
        let tmp = tempfile::tempdir().unwrap();
        let task = Task {
            id: "TASK-200".to_string(),
            title: "Link test".to_string(),
            status: "todo".to_string(),
            priority: None,
            execution: Some(crate::task::TaskExecution {
                stages: vec![
                    "00_first".to_string(),
                    "01_second".to_string(),
                ],
                strategy: None,
            }),
            acceptance_criteria: vec![],
            description: "Link test".to_string(),
            source_path: None,
        };

        let wf = Workflow::init(&task, tmp.path()).await.unwrap();
        assert_eq!(wf.stages.len(), 2);

        let first_input = wf.stages[0].path.join("input");
        assert!(tokio::fs::try_exists(&first_input).await.unwrap());

        let first_output = wf.stages[0].path.join("output");
        let second_input = wf.stages[1].path.join("input");

        #[cfg(unix)]
        {
            let link_target = tokio::fs::read_link(&second_input).await.unwrap();
            assert_eq!(link_target, first_output);
        }

        #[cfg(windows)]
        {
            tokio::fs::write(first_output.join("test.txt"), "hello")
                .await
                .unwrap();
            let content = tokio::fs::read_to_string(second_input.join("test.txt"))
                .await
                .unwrap();
            assert_eq!(content, "hello");
        }
    }
}
