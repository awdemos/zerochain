use std::collections::BTreeMap;

use crate::stage::{Stage, StageId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageState {
    Pending,
    Ready,
    Running,
    Complete,
    Error,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StageNode {
    pub id: StageId,
    pub dependencies: Vec<StageId>,
    pub state: StageState,
    pub human_gate: bool,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StageGroup {
    pub group_key: String,
    pub stages: Vec<StageId>,
    pub state: StageState,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ExecutionPlan {
    pub groups: Vec<StageGroup>,
    pub stage_map: BTreeMap<String, StageNode>,
}

impl ExecutionPlan {
    #[must_use] pub fn from_stages(stages: &[Stage]) -> Self {
        let mut groups: BTreeMap<String, Vec<StageId>> = BTreeMap::new();
        let mut stage_map = BTreeMap::new();

        for stage in stages {
            let group_key = stage.id.parallel_group().map_or_else(
                || stage.id.raw.clone(),
                |_| {
                    let raw = &stage.id.raw;
                    let prefix: String = raw.chars().take_while(char::is_ascii_digit).collect();
                    format!("{prefix}_parallel")
                },
            );

            let state = if stage.is_error {
                StageState::Error
            } else if stage.is_complete {
                StageState::Complete
            } else {
                StageState::Pending
            };

            stage_map.insert(
                stage.id.raw.clone(),
                StageNode {
                    id: stage.id.clone(),
                    dependencies: Vec::new(),
                    state,
                    human_gate: stage.human_gate,
                },
            );

            groups.entry(group_key).or_default().push(stage.id.clone());
        }

        let sorted_group_keys: Vec<String> = {
            let mut keys: Vec<String> = groups.keys().cloned().collect();
            keys.sort_by(|a, b| {
                let get_sort_key = |k: &str| -> (u32, String) {
                    let digits: String = k.chars().take_while(char::is_ascii_digit).collect();
                    let seq: u32 = digits.parse().unwrap_or(0);
                    (seq, k.to_string())
                };
                get_sort_key(a).cmp(&get_sort_key(b))
            });
            keys
        };

        let group_list: Vec<StageGroup> = sorted_group_keys
            .into_iter()
            .map(|key| {
                let stage_ids_in_group = &groups[&key];
                let all_complete = stage_ids_in_group
                    .iter()
                    .all(|id| matches!(stage_map[&id.raw].state, StageState::Complete));
                let any_error = stage_ids_in_group
                    .iter()
                    .any(|id| matches!(stage_map[&id.raw].state, StageState::Error));

                let state = if any_error {
                    StageState::Error
                } else if all_complete {
                    StageState::Complete
                } else {
                    StageState::Pending
                };

                StageGroup {
                    group_key: key,
                    stages: stage_ids_in_group.clone(),
                    state,
                }
            })
            .collect();

        for (i, group) in group_list.iter().enumerate() {
            if i > 0 {
                let prev_group = &group_list[i - 1];
                for stage_id in &group.stages {
                    for dep_id in &prev_group.stages {
                        if let Some(node) = stage_map.get_mut(&stage_id.raw) {
                            node.dependencies.push(dep_id.clone());
                        }
                    }
                }
            }
        }

        ExecutionPlan {
            groups: group_list,
            stage_map,
        }
    }

    #[must_use] pub fn next_stage(&self) -> Option<&StageId> {
        for group in &self.groups {
            match group.state {
                StageState::Error => return None,
                StageState::Complete => {},
                _ => {
                    for stage_id in &group.stages {
                        let node = &self.stage_map[&stage_id.raw];
                        if matches!(node.state, StageState::Pending) {
                            let deps_satisfied = node.dependencies.iter().all(|dep_id| {
                                matches!(self.stage_map[&dep_id.raw].state, StageState::Complete)
                            });
                            if deps_satisfied {
                                return Some(stage_id);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    #[must_use] pub fn is_complete(&self) -> bool {
        self.groups
            .iter()
            .all(|g| matches!(g.state, StageState::Complete))
    }

    pub fn mark_complete(&mut self, stage_id: &StageId) {
        if let Some(node) = self.stage_map.get_mut(&stage_id.raw) {
            node.state = StageState::Complete;
        }

        for group in &mut self.groups {
            if group.stages.iter().any(|s| s == stage_id) {
                let all_done = group
                    .stages
                    .iter()
                    .all(|s| matches!(self.stage_map[&s.raw].state, StageState::Complete));
                if all_done {
                    group.state = StageState::Complete;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_stage(raw: &str, complete: bool, error: bool) -> Stage {
        let id = StageId::parse(raw).unwrap();
        Stage {
            id,
            path: PathBuf::from(raw),
            context_path: PathBuf::from(raw).join("CONTEXT.md"),
            input_path: PathBuf::from(raw).join("input"),
            output_path: PathBuf::from(raw).join("output"),
            is_complete: complete,
            is_error: error,
            human_gate: false,
            container_image: None,
            command: None,
        }
    }

    #[test]
    fn sequential_plan() {
        let stages = vec![
            make_stage("00_spec", false, false),
            make_stage("01_analyze", false, false),
            make_stage("02_implement", false, false),
        ];

        let plan = ExecutionPlan::from_stages(&stages);
        assert_eq!(plan.groups.len(), 3);
        assert!(!plan.is_complete());

        let next = plan.next_stage().unwrap();
        assert_eq!(next.raw, "00_spec");
    }

    #[test]
    fn plan_progression() {
        let stages = vec![
            make_stage("00_spec", false, false),
            make_stage("01_analyze", false, false),
            make_stage("02_implement", false, false),
        ];

        let mut plan = ExecutionPlan::from_stages(&stages);

        let first = plan.next_stage().unwrap().clone();
        plan.mark_complete(&first);

        let second = plan.next_stage().unwrap().clone();
        assert_eq!(second.raw, "01_analyze");
        plan.mark_complete(&second);

        let third = plan.next_stage().unwrap().clone();
        assert_eq!(third.raw, "02_implement");
        plan.mark_complete(&third);

        assert!(plan.is_complete());
        assert!(plan.next_stage().is_none());
    }

    #[test]
    fn parallel_stages_grouped() {
        let stages = vec![
            make_stage("00_spec", false, false),
            make_stage("01a_test_unit", false, false),
            make_stage("01b_test_integration", false, false),
            make_stage("02_deploy", false, false),
        ];

        let plan = ExecutionPlan::from_stages(&stages);

        assert_eq!(plan.groups.len(), 3);

        let parallel_group = &plan.groups[1];
        assert_eq!(parallel_group.stages.len(), 2);
        assert!(parallel_group
            .stages
            .iter()
            .any(|s| s.raw == "01a_test_unit"));
        assert!(parallel_group
            .stages
            .iter()
            .any(|s| s.raw == "01b_test_integration"));

        let first = plan.next_stage().unwrap().clone();
        assert_eq!(first.raw, "00_spec");
    }

    #[test]
    fn error_blocks_further_execution() {
        let stages = vec![
            make_stage("00_spec", true, false),
            make_stage("01_analyze", false, true),
            make_stage("02_implement", false, false),
        ];

        let plan = ExecutionPlan::from_stages(&stages);
        assert!(plan.next_stage().is_none());
    }

    #[test]
    fn completed_workflow() {
        let stages = vec![
            make_stage("00_spec", true, false),
            make_stage("01_analyze", true, false),
        ];

        let plan = ExecutionPlan::from_stages(&stages);
        assert!(plan.is_complete());
        assert!(plan.next_stage().is_none());
    }
}
