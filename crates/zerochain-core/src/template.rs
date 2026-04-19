use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct StageDef {
    pub name: String,
    pub role: String,
    pub body: String,
    pub human_gate: bool,
}

impl StageDef {
    pub fn to_context_md(&self) -> String {
        let mut frontmatter = format!("---\nrole: {}", self.role);
        if self.human_gate {
            frontmatter.push_str("\nhuman_gate: true");
        }
        frontmatter.push_str("\n---\n\n");
        format!("{frontmatter}{}\n", self.body)
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Template {
    pub name: String,
    pub description: String,
    pub stages: Vec<StageDef>,
}

impl Template {
    pub fn stage_names(&self) -> Vec<String> {
        self.stages.iter().map(|s| s.name.clone()).collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct TemplateRegistry {
    templates: HashMap<String, Template>,
}

#[derive(Debug, serde::Deserialize)]
struct TemplateToml {
    name: String,
    description: String,
    stages: HashMap<String, StageToml>,
}

#[derive(Debug, serde::Deserialize)]
struct StageToml {
    role: String,
    #[serde(default)]
    human_gate: bool,
    #[serde(default)]
    body: String,
}

impl TemplateRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        registry.register_builtins();
        registry
    }

    pub fn get(&self, name: &str) -> Option<&Template> {
        self.templates.get(name)
    }

    pub fn list(&self) -> Vec<&Template> {
        let mut list: Vec<_> = self.templates.values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    pub fn load_from_dir(&mut self, dir: &Path) -> Result<(), LoadFromDirError> {
        if !dir.is_dir() {
            return Err(LoadFromDirError::NotADirectory(dir.to_path_buf()));
        }
        let entries = std::fs::read_dir(dir).map_err(LoadFromDirError::Io)?;
        for entry in entries {
            let entry = entry.map_err(LoadFromDirError::Io)?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest = path.join("template.toml");
            if !manifest.exists() {
                continue;
            }
            let raw = std::fs::read_to_string(&manifest).map_err(LoadFromDirError::Io)?;
            let parsed: TemplateToml =
                toml::from_str(&raw).map_err(|e| LoadFromDirError::Parse {
                    path: manifest.clone(),
                    source: e,
                })?;

            let mut stages: Vec<StageDef> = parsed
                .stages
                .into_iter()
                .map(|(name, s)| StageDef {
                    name,
                    role: s.role,
                    body: s.body,
                    human_gate: s.human_gate,
                })
                .collect();
            stages.sort_by(|a, b| a.name.cmp(&b.name));

            self.register(Template {
                name: parsed.name,
                description: parsed.description,
                stages,
            });
        }
        Ok(())
    }

    fn register(&mut self, template: Template) {
        self.templates.insert(template.name.clone(), template);
    }

    fn register_builtins(&mut self) {
        self.register(Template {
            name: "code-review".into(),
            description: "Review code for correctness, style, and security".into(),
            stages: vec![
                StageDef {
                    name: "00_spec".into(),
                    role: "senior developer".into(),
                    body: "Describe the code in the input directory. Identify the main components, \
                           their responsibilities, and how they interact. Output a summary to result.md."
                        .into(),
                    human_gate: false,
                },
                StageDef {
                    name: "01_review".into(),
                    role: "senior code reviewer".into(),
                    body: "Review the code described in the previous stage. Check for: correctness, \
                           performance issues, security vulnerabilities, and style violations. \
                           Output findings to result.md."
                        .into(),
                    human_gate: false,
                },
                StageDef {
                    name: "02_report".into(),
                    role: "technical writer".into(),
                    body: "Synthesize the code review findings into a clear, actionable report. \
                           Prioritize issues by severity. Output the report to result.md."
                        .into(),
                    human_gate: true,
                },
            ],
        });

        self.register(Template {
            name: "research".into(),
            description: "Investigate a question and synthesize findings".into(),
            stages: vec![
                StageDef {
                    name: "00_question".into(),
                    role: "research analyst".into(),
                    body: "Define the research question precisely. Break it down into sub-questions \
                           and identify what information is needed. Output the analysis plan to result.md."
                        .into(),
                    human_gate: false,
                },
                StageDef {
                    name: "01_research".into(),
                    role: "research analyst".into(),
                    body: "Investigate each sub-question using the provided input. Gather evidence, \
                           note sources, and identify patterns. Output raw findings to result.md."
                        .into(),
                    human_gate: false,
                },
                StageDef {
                    name: "02_synthesize".into(),
                    role: "research analyst".into(),
                    body: "Synthesize the research findings into a coherent answer to the original \
                           question. Highlight key insights and remaining uncertainties. Output to result.md."
                        .into(),
                    human_gate: true,
                },
            ],
        });

        self.register(Template {
            name: "implement".into(),
            description: "Design and implement a feature with verification".into(),
            stages: vec![
                StageDef {
                    name: "00_spec".into(),
                    role: "product engineer".into(),
                    body: "Analyze the requirements in the input. Define clear acceptance criteria, \
                           edge cases, and constraints. Output the specification to result.md."
                        .into(),
                    human_gate: false,
                },
                StageDef {
                    name: "01_design".into(),
                    role: "software architect".into(),
                    body: "Design the solution based on the specification. Define the architecture, \
                           key data structures, interfaces, and error handling strategy. \
                           Output the design document to result.md."
                        .into(),
                    human_gate: true,
                },
                StageDef {
                    name: "02_implement".into(),
                    role: "senior developer".into(),
                    body: "Implement the solution following the design document. Write clean, \
                           well-tested code. Follow existing codebase patterns and conventions. \
                           Output the implementation to result.md."
                        .into(),
                    human_gate: false,
                },
                StageDef {
                    name: "03_verify".into(),
                    role: "QA engineer".into(),
                    body: "Verify the implementation against the acceptance criteria. Run tests, \
                           check edge cases, and validate error handling. Output the verification \
                           report to result.md."
                        .into(),
                    human_gate: true,
                },
            ],
        });
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadFromDirError {
    #[error("not a directory: {}", .0.display())]
    NotADirectory(std::path::PathBuf),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error in {}: {source}", .path.display())]
    Parse {
        path: std::path::PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_builtins() {
        let reg = TemplateRegistry::new();
        assert!(reg.get("code-review").is_some());
        assert!(reg.get("research").is_some());
        assert!(reg.get("implement").is_some());
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn list_returns_sorted() {
        let reg = TemplateRegistry::new();
        let list = reg.list();
        let names: Vec<&str> = list.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["code-review", "implement", "research"]);
    }

    #[test]
    fn code_review_template_stages() {
        let reg = TemplateRegistry::new();
        let tpl = reg.get("code-review").unwrap();
        assert_eq!(tpl.stages.len(), 3);
        assert_eq!(tpl.stages[0].name, "00_spec");
        assert!(tpl.stages[2].human_gate);
    }

    #[test]
    fn research_template_stages() {
        let reg = TemplateRegistry::new();
        let tpl = reg.get("research").unwrap();
        assert_eq!(tpl.stages.len(), 3);
        assert_eq!(tpl.stages[2].name, "02_synthesize");
    }

    #[test]
    fn implement_template_stages() {
        let reg = TemplateRegistry::new();
        let tpl = reg.get("implement").unwrap();
        assert_eq!(tpl.stages.len(), 4);
        assert!(tpl.stages[1].human_gate);
        assert!(tpl.stages[3].human_gate);
    }

    #[test]
    fn stage_names_method() {
        let reg = TemplateRegistry::new();
        let tpl = reg.get("code-review").unwrap();
        assert_eq!(
            tpl.stage_names(),
            vec!["00_spec", "01_review", "02_report"]
        );
    }

    #[test]
    fn to_context_md_without_human_gate() {
        let stage = StageDef {
            name: "00_spec".into(),
            role: "analyst".into(),
            body: "Do the thing.".into(),
            human_gate: false,
        };
        let md = stage.to_context_md();
        assert!(md.starts_with("---\nrole: analyst\n---"));
        assert!(!md.contains("human_gate"));
        assert!(md.contains("Do the thing."));
    }

    #[test]
    fn to_context_md_with_human_gate() {
        let stage = StageDef {
            name: "02_report".into(),
            role: "writer".into(),
            body: "Write report.".into(),
            human_gate: true,
        };
        let md = stage.to_context_md();
        assert!(md.contains("human_gate: true"));
    }

    #[test]
    fn stage_def_includes_newline_at_end() {
        let stage = StageDef {
            name: "00_test".into(),
            role: "tester".into(),
            body: "Test things.".into(),
            human_gate: false,
        };
        let md = stage.to_context_md();
        assert!(md.ends_with("Test things.\n"));
    }

    #[test]
    fn load_from_dir_reads_template_toml() {
        let dir = tempfile::tempdir().unwrap();
        let tpl_dir = dir.path().join("custom-task");
        std::fs::create_dir_all(&tpl_dir).unwrap();
        std::fs::write(
            tpl_dir.join("template.toml"),
            r#"
name = "custom-task"
description = "A custom task template"

[stages."00_analyze"]
role = "analyst"
body = "Analyze input."

[stages."01_execute"]
role = "executor"
body = "Execute the plan."
human_gate = true
"#,
        )
        .unwrap();

        let mut reg = TemplateRegistry::new();
        reg.load_from_dir(dir.path()).unwrap();

        let tpl = reg.get("custom-task").expect("custom-task should be loaded");
        assert_eq!(tpl.description, "A custom task template");
        assert_eq!(tpl.stages.len(), 2);
        assert_eq!(tpl.stages[0].name, "00_analyze");
        assert!(!tpl.stages[0].human_gate);
        assert_eq!(tpl.stages[1].name, "01_execute");
        assert!(tpl.stages[1].human_gate);
    }

    #[test]
    fn load_from_dir_skips_dirs_without_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let empty_dir = dir.path().join("no-manifest");
        std::fs::create_dir_all(&empty_dir).unwrap();

        let mut reg = TemplateRegistry::new();
        reg.load_from_dir(dir.path()).unwrap();
        assert!(reg.get("no-manifest").is_none());
    }

    #[test]
    fn load_from_dir_overrides_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let tpl_dir = dir.path().join("code-review");
        std::fs::create_dir_all(&tpl_dir).unwrap();
        std::fs::write(
            tpl_dir.join("template.toml"),
            r#"
name = "code-review"
description = "Overridden code review"

[stages."00_check"]
role = "checker"
body = "Check things."
"#,
        )
        .unwrap();

        let mut reg = TemplateRegistry::new();
        reg.load_from_dir(dir.path()).unwrap();

        let tpl = reg.get("code-review").unwrap();
        assert_eq!(tpl.description, "Overridden code review");
        assert_eq!(tpl.stages.len(), 1);
    }

    #[test]
    fn load_from_dir_rejects_nonexistent() {
        let mut reg = TemplateRegistry::new();
        let result = reg.load_from_dir(Path::new("/no/such/path"));
        assert!(result.is_err());
    }

    #[test]
    fn load_from_dir_reports_bad_toml() {
        let dir = tempfile::tempdir().unwrap();
        let tpl_dir = dir.path().join("bad");
        std::fs::create_dir_all(&tpl_dir).unwrap();
        std::fs::write(tpl_dir.join("template.toml"), "not valid toml {{{").unwrap();

        let mut reg = TemplateRegistry::new();
        let result = reg.load_from_dir(dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("parse error"), "expected parse error, got: {err}");
    }

    #[test]
    fn load_from_dir_stages_sorted_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let tpl_dir = dir.path().join("sorted");
        std::fs::create_dir_all(&tpl_dir).unwrap();
        std::fs::write(
            tpl_dir.join("template.toml"),
            r#"
name = "sorted"
description = "test ordering"

[stages."02_second"]
role = "worker"
body = "Second step."

[stages."00_first"]
role = "worker"
body = "First step."

[stages."01_middle"]
role = "worker"
body = "Middle step."
"#,
        )
        .unwrap();

        let mut reg = TemplateRegistry::new();
        reg.load_from_dir(dir.path()).unwrap();

        let tpl = reg.get("sorted").unwrap();
        assert_eq!(
            tpl.stage_names(),
            vec!["00_first", "01_middle", "02_second"]
        );
    }
}
