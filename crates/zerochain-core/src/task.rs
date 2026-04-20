use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{io_err, Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct TaskExecution {
    #[serde(default)]
    pub stages: Vec<String>,
    #[serde(default)]
    pub strategy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub execution: Option<TaskExecution>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(skip)]
    pub description: String,
    #[serde(skip)]
    pub source_path: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct TaskFrontmatter {
    id: String,
    title: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    execution: Option<TaskExecution>,
    #[serde(default)]
    acceptance_criteria: Vec<String>,
}

impl Task {
    pub async fn from_file(path: &Path) -> Result<Self> {
        let content = tokio::fs::read_to_string(path).await.map_err(|e| io_err(path.to_path_buf(), e))?;

        let mut task = Self::parse(&content)?;
        task.source_path = Some(path.to_path_buf());
        Ok(task)
    }

    pub fn parse(content: &str) -> Result<Self> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return Err(Error::TaskParse {
                path: std::path::PathBuf::from("<inline>"),
                reason: "missing YAML frontmatter".to_string(),
            });
        }

        let after_first = &trimmed[3..];

        let end_marker = after_first.find("\n---").ok_or_else(|| Error::TaskParse {
            path: std::path::PathBuf::from("<inline>"),
            reason: "unclosed YAML frontmatter (missing closing ---)".to_string(),
        })?;

        let yaml_str = &after_first[..end_marker];
        let description = after_first[end_marker + 4..].trim_start().to_string();

        let fm: TaskFrontmatter = serde_yaml::from_str(yaml_str).map_err(|e| Error::TaskParse {
            path: std::path::PathBuf::from("<inline>"),
            reason: format!("YAML parse error: {e}"),
        })?;

        Ok(Task {
            id: fm.id,
            title: fm.title,
            status: fm.status,
            priority: fm.priority,
            execution: fm.execution,
            acceptance_criteria: fm.acceptance_criteria,
            description,
            source_path: None,
        })
    }

    pub fn stage_names(&self) -> Vec<String> {
        self.execution
            .as_ref()
            .map(|e| e.stages.clone())
            .unwrap_or_default()
    }
}

impl TaskExecution {
    pub fn new(stages: Vec<String>, strategy: Option<String>) -> Self {
        Self { stages, strategy }
    }
}

impl Task {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        title: String,
        status: String,
        priority: Option<String>,
        execution: Option<TaskExecution>,
        acceptance_criteria: Vec<String>,
        description: String,
        source_path: Option<std::path::PathBuf>,
    ) -> Self {
        Task {
            id,
            title,
            status,
            priority,
            execution,
            acceptance_criteria,
            description,
            source_path,
        }
    }

    pub fn builder(id: impl Into<String>, title: impl Into<String>) -> TaskBuilder {
        TaskBuilder::new(id, title)
    }
}

/// Builder for [`Task`].
#[derive(Debug, Clone)]
pub struct TaskBuilder {
    id: String,
    title: String,
    status: String,
    priority: Option<String>,
    execution: Option<TaskExecution>,
    acceptance_criteria: Vec<String>,
    description: String,
    source_path: Option<std::path::PathBuf>,
}

impl TaskBuilder {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            status: "todo".into(),
            priority: None,
            execution: None,
            acceptance_criteria: Vec::new(),
            description: String::new(),
            source_path: None,
        }
    }

    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = status.into();
        self
    }

    pub fn priority(mut self, priority: impl Into<String>) -> Self {
        self.priority = Some(priority.into());
        self
    }

    pub fn execution(mut self, execution: TaskExecution) -> Self {
        self.execution = Some(execution);
        self
    }

    pub fn stages(mut self, stages: Vec<String>) -> Self {
        self.execution = Some(TaskExecution::new(stages, Some("sequential".into())));
        self
    }

    pub fn acceptance_criteria(mut self, criteria: Vec<String>) -> Self {
        self.acceptance_criteria = criteria;
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn source_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.source_path = Some(path.into());
        self
    }

    pub fn build(self) -> Task {
        Task {
            id: self.id,
            title: self.title,
            status: self.status,
            priority: self.priority,
            execution: self.execution,
            acceptance_criteria: self.acceptance_criteria,
            description: self.description,
            source_path: self.source_path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_task_content() -> &'static str {
        r#"---
id: TASK-001
title: Implement authentication
status: todo
priority: high
execution:
  stages:
    - 00_spec
    - 01_analyze
    - 02_implement
  strategy: sequential
acceptance_criteria:
  - User can log in
  - Session tokens are validated
---
# Task Description

This task implements the full authentication flow including
login, token management, and session validation.
"#
    }

    #[test]
    fn parse_full_task() {
        let task = Task::parse(sample_task_content()).unwrap();
        assert_eq!(task.id, "TASK-001");
        assert_eq!(task.title, "Implement authentication");
        assert_eq!(task.status, "todo");
        assert_eq!(task.priority.as_deref(), Some("high"));
        assert_eq!(
            task.stage_names(),
            vec!["00_spec", "01_analyze", "02_implement"]
        );

        let exec = task.execution.unwrap();
        assert_eq!(exec.strategy.as_deref(), Some("sequential"));

        assert_eq!(task.acceptance_criteria.len(), 2);
        assert!(task.description.contains("authentication flow"));
    }

    #[test]
    fn parse_minimal_task() {
        let input = "---\nid: TASK-002\ntitle: Minimal task\n---\nSome description";
        let task = Task::parse(input).unwrap();
        assert_eq!(task.id, "TASK-002");
        assert_eq!(task.title, "Minimal task");
        assert_eq!(task.status, "");
        assert!(task.priority.is_none());
        assert!(task.execution.is_none());
        assert!(task.stage_names().is_empty());
        assert_eq!(task.description, "Some description");
    }

    #[test]
    fn reject_no_frontmatter() {
        let input = "Just markdown, no frontmatter";
        assert!(Task::parse(input).is_err());
    }

    #[test]
    fn reject_unclosed_frontmatter() {
        let input = "---\nid: TASK-003\ntitle: Test\n";
        assert!(Task::parse(input).is_err());
    }
}
