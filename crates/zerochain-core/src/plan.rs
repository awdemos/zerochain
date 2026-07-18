use std::collections::BTreeMap;

use crate::graph::{
    ControlOutcome, LoopExhaustion, Node, NodeId, StageGraphBuilder, WorkflowGraph,
};
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

/// Execution plan backed by an explicit `WorkflowGraph`.
///
/// The graph replaces the old hand-rolled `StageId`-convention dependency
/// inference. The public API (`groups`, `stage_map`, `next_stage`,
/// `is_complete`, `mark_complete`) is preserved so callers in `zerochain-engine`
/// do not need to change.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ExecutionPlan {
    pub groups: Vec<StageGroup>,
    pub stage_map: BTreeMap<String, StageNode>,
    graph: WorkflowGraph,
    /// Tracks how many iterations each loop node has executed.
    loop_iterations: BTreeMap<NodeId, usize>,
    /// Tracks whether a loop has terminated early and with what outcome.
    loop_outcome: BTreeMap<NodeId, ControlOutcome>,
}

impl ExecutionPlan {
    #[must_use]
    pub fn from_stages(stages: &[Stage]) -> Self {
        let graph = StageGraphBuilder::from_stages(stages);
        Self::from_graph(stages, graph)
    }

    /// Build a plan from an explicit graph. Useful for tests and for callers
    /// that construct graphs with loops or custom edges.
    #[must_use]
    pub fn from_graph(stages: &[Stage], graph: WorkflowGraph) -> Self {
        let mut stage_map: BTreeMap<String, StageNode> = BTreeMap::new();
        let mut groups: BTreeMap<String, Vec<StageId>> = BTreeMap::new();
        let loop_iterations: BTreeMap<NodeId, usize> = BTreeMap::new();
        let mut loop_outcome: BTreeMap<NodeId, ControlOutcome> = BTreeMap::new();

        // Compute a stable topological order to derive legacy group keys.
        let order = graph
            .topological_order()
            .unwrap_or_else(|_| graph.nodes().keys().copied().collect::<Vec<_>>());

        for node_id in order {
            let Some(node) = graph.get(node_id) else {
                continue;
            };

            let stage_id = node.stage_id().clone();
            let stage = stages.iter().find(|s| s.id == stage_id);

            let state = if stage.is_some_and(|s| s.is_error) {
                StageState::Error
            } else if stage.is_some_and(|s| s.is_complete) {
                StageState::Complete
            } else {
                StageState::Pending
            };

            let dependencies: Vec<StageId> = node
                .dependencies()
                .iter()
                .filter_map(|dep_id| {
                    graph
                        .get(*dep_id)
                        .map(|dep_node| dep_node.stage_id().clone())
                })
                .collect();

            stage_map.insert(
                stage_id.raw.clone(),
                StageNode {
                    id: stage_id.clone(),
                    dependencies,
                    state,
                    human_gate: stage.map(|s| s.human_gate).unwrap_or(false),
                },
            );

            // Read any persisted control record from the stage directory. This
            // lets loops terminate durably across actor messages.
            if let Some(stage) = stage {
                let control_path = stage.path.join(".control");
                if let Ok(content) = std::fs::read_to_string(&control_path) {
                    if let Some(outcome) = ControlOutcome::parse_record(&content) {
                        for loop_id in graph.loops_for_body(&stage.id) {
                            loop_outcome.insert(loop_id, outcome);
                        }
                    }
                }
            }

            let group_key = stage_id.parallel_group().map_or_else(
                || stage_id.raw.clone(),
                |_| {
                    let raw = &stage_id.raw;
                    let prefix: String = raw.chars().take_while(char::is_ascii_digit).collect();
                    format!("{prefix}_parallel")
                },
            );

            groups.entry(group_key).or_default().push(stage_id);
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

        Self {
            groups: group_list,
            stage_map,
            graph,
            loop_iterations,
            loop_outcome,
        }
    }

    #[must_use]
    pub fn next_stage(&self) -> Option<&StageId> {
        // Walk the graph in topological order so loop nodes are reached even
        // when their group state is not yet computed by the legacy group logic.
        let order = self
            .graph
            .topological_order()
            .unwrap_or_else(|_| self.graph.nodes().keys().copied().collect::<Vec<_>>());

        for node_id in order {
            let node = self.graph.get(node_id)?;
            let node_state = self.node_state(node_id);

            if !matches!(node_state, StageState::Pending) {
                continue;
            }

            let deps_satisfied = node
                .dependencies()
                .iter()
                .all(|dep_id| matches!(self.node_state(*dep_id), StageState::Complete));

            if !deps_satisfied {
                continue;
            }

            return Some(self.resolve_runnable_stage_id(node_id));
        }
        None
    }

    fn resolve_runnable_stage_id(&self, node_id: NodeId) -> &StageId {
        match self.graph.get(node_id) {
            Some(Node::Loop { body, .. }) => self
                .graph
                .get(*body)
                .map(|n| n.stage_id())
                .unwrap_or_else(|| self.graph.get(node_id).unwrap().stage_id()),
            Some(node) => node.stage_id(),
            None => unreachable!("node_id {node_id} was retrieved from graph earlier"),
        }
    }

    fn node_state(&self, node_id: NodeId) -> StageState {
        match self.graph.get(node_id) {
            Some(Node::Loop {
                body,
                max_iterations,
                on_exhausted,
                ..
            }) => {
                let body_state = self.node_state(*body);
                let iterations = self.loop_iterations.get(&node_id).copied().unwrap_or(0);
                let outcome = self.loop_outcome.get(&node_id).copied();

                if let Some(outcome) = outcome {
                    return match outcome {
                        ControlOutcome::Return => StageState::Complete,
                        ControlOutcome::Escalate => StageState::Complete,
                        ControlOutcome::Fail => StageState::Error,
                        ControlOutcome::Await => StageState::Running,
                        ControlOutcome::Continue => StageState::Running,
                    };
                }

                if iterations >= *max_iterations {
                    return match on_exhausted {
                        LoopExhaustion::Fail => StageState::Error,
                        LoopExhaustion::Succeed => StageState::Complete,
                    };
                }

                // Loop is pending if its body is pending and we have not started.
                if iterations == 0 && matches!(body_state, StageState::Pending) {
                    return StageState::Pending;
                }

                // Loop is running while its body is iterating.
                StageState::Running
            }
            Some(Node::Stage { stage_id, .. }) => {
                let own_state = self
                    .stage_map
                    .get(&stage_id.raw)
                    .map(|n| n.state.clone())
                    .unwrap_or(StageState::Pending);
                self.enclosing_loop_state(stage_id).unwrap_or(own_state)
            }
            None => StageState::Pending,
        }
    }

    fn enclosing_loop_state(&self, stage_id: &StageId) -> Option<StageState> {
        let node_id = self.graph.node_id_for_stage(stage_id)?;
        let loop_node = self
            .graph
            .nodes()
            .values()
            .find(|n| matches!(n, Node::Loop { body: b, .. } if *b == node_id))?;
        let loop_id = loop_node.id();
        if let Some(outcome) = self.loop_outcome.get(&loop_id).copied() {
            return match outcome {
                ControlOutcome::Return | ControlOutcome::Escalate => Some(StageState::Complete),
                ControlOutcome::Fail => Some(StageState::Error),
                ControlOutcome::Await | ControlOutcome::Continue => None,
            };
        }
        let iterations = self.loop_iterations.get(&loop_id).copied().unwrap_or(0);
        let max_iterations = match loop_node {
            Node::Loop { max_iterations, .. } => *max_iterations,
            _ => return None,
        };
        if iterations >= max_iterations {
            return match loop_node {
                Node::Loop {
                    on_exhausted: LoopExhaustion::Fail,
                    ..
                } => Some(StageState::Error),
                Node::Loop {
                    on_exhausted: LoopExhaustion::Succeed,
                    ..
                } => Some(StageState::Complete),
                _ => None,
            };
        }
        None
    }

    pub fn mark_complete(&mut self, stage_id: &StageId) {
        if let Some(node_id) = self.graph.node_id_for_stage(stage_id) {
            if let Some(Node::Loop { .. }) = self.graph.get(node_id) {
                self.loop_iterations
                    .entry(node_id)
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
                return;
            }
        }

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

        self.propagate_loop_completion(stage_id);
    }

    pub fn mark_error(&mut self, stage_id: &StageId) {
        if let Some(node_id) = self.graph.node_id_for_stage(stage_id) {
            if let Some(Node::Loop { .. }) = self.graph.get(node_id) {
                self.loop_outcome.insert(node_id, ControlOutcome::Fail);
                return;
            }
        }

        if let Some(node) = self.stage_map.get_mut(&stage_id.raw) {
            node.state = StageState::Error;
        }

        for group in &mut self.groups {
            if group.stages.iter().any(|s| s == stage_id) {
                group.state = StageState::Error;
            }
        }

        self.propagate_loop_completion(stage_id);
    }

    /// Record a control outcome emitted by a stage. If the stage is the body of
    /// a loop, the outcome controls loop termination. Otherwise it is ignored
    /// by the legacy group logic (future scheduling may act on it).
    pub fn record_control_outcome(&mut self, stage_id: &StageId, outcome: ControlOutcome) {
        let Some(node_id) = self.graph.node_id_for_stage(stage_id) else {
            return;
        };

        // Find any loop whose body includes this stage. For now, loops wrap a
        // single body node, so the stage is either a loop node itself or the
        // body entry.
        if let Some(Node::Loop { id: loop_id, .. }) = self
            .graph
            .nodes()
            .values()
            .find(|n| matches!(n, Node::Loop { body: b, .. } if *b == node_id))
        {
            self.loop_outcome.insert(*loop_id, outcome);
            if outcome != ControlOutcome::Continue {
                self.loop_iterations
                    .entry(*loop_id)
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
            }
        }
    }

    fn propagate_loop_completion(&mut self, stage_id: &StageId) {
        let Some(completed_node_id) = self.graph.node_id_for_stage(stage_id) else {
            return;
        };

        for node in self.graph.nodes().values() {
            let Node::Loop {
                id: loop_id,
                body,
                max_iterations,
                ..
            } = node
            else {
                continue;
            };
            if *body != completed_node_id {
                continue;
            }
            if self.loop_outcome.contains_key(loop_id) {
                continue;
            }
            let current = self.loop_iterations.get(loop_id).copied().unwrap_or(0);
            if current >= *max_iterations {
                continue;
            }
            self.loop_iterations.insert(*loop_id, current + 1);
            if let Some(body_node) = self.graph.get(*body) {
                let body_stage_id = body_node.stage_id().raw.clone();
                if let Some(n) = self.stage_map.get_mut(&body_stage_id) {
                    n.state = StageState::Pending;
                }
            }
        }
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        // Use the graph truth when loops are involved; otherwise fall back to
        // the legacy group check for backwards compatibility.
        if self.graph.nodes().is_empty() {
            return self
                .groups
                .iter()
                .all(|g| matches!(g.state, StageState::Complete));
        }

        let order = self
            .graph
            .topological_order()
            .unwrap_or_else(|_| self.graph.nodes().keys().copied().collect::<Vec<_>>());
        order
            .iter()
            .all(|id| matches!(self.node_state(*id), StageState::Complete))
    }

    #[must_use]
    pub fn graph(&self) -> &WorkflowGraph {
        &self.graph
    }

    pub fn should_reset_body_for_loop_iteration(&self, stage_id: &StageId) -> bool {
        let Some(node_id) = self.graph.node_id_for_stage(stage_id) else {
            return false;
        };

        let loop_node = self
            .graph
            .nodes()
            .values()
            .find(|n| matches!(n, Node::Loop { body: b, .. } if *b == node_id));

        let Some(Node::Loop { id: loop_id, .. }) = loop_node else {
            return false;
        };

        let has_started = self.loop_iterations.get(loop_id).copied().unwrap_or(0) > 0;
        let not_terminal = !self.loop_outcome.contains_key(loop_id);
        has_started && not_terminal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::graph::{LoopExhaustion, WorkflowGraph};

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

    #[test]
    fn loop_runs_body_repeatedly_until_return() {
        let mut graph = WorkflowGraph::new();
        let worker = graph.add_stage(StageId::parse("00_worker").unwrap());
        graph
            .add_loop(
                StageId::parse("01_loop").unwrap(),
                worker,
                5,
                LoopExhaustion::Fail,
            )
            .unwrap();
        // The loop node depends on the worker setup stage, but here it is the
        // root node.

        let stages = vec![
            make_stage("00_worker", false, false),
            make_stage("01_loop", false, false),
        ];
        let mut plan = ExecutionPlan::from_graph(&stages, graph);

        // First pass: loop is pending, body runs.
        let next = plan.next_stage().unwrap();
        assert_eq!(next.raw, "00_worker");
        plan.mark_complete(&StageId::parse("00_worker").unwrap());

        // Loop increments iteration and resets body to pending.
        let next = plan.next_stage().unwrap();
        assert_eq!(next.raw, "00_worker");

        // Body emits return control record.
        plan.record_control_outcome(
            &StageId::parse("00_worker").unwrap(),
            ControlOutcome::Return,
        );
        plan.mark_complete(&StageId::parse("00_worker").unwrap());

        // Loop is now complete.
        assert!(plan.is_complete());
        assert!(plan.next_stage().is_none());
    }

    #[test]
    fn loop_exhaustion_fails_when_configured() {
        let mut graph = WorkflowGraph::new();
        let worker = graph.add_stage(StageId::parse("00_worker").unwrap());
        graph
            .add_loop(
                StageId::parse("01_loop").unwrap(),
                worker,
                2,
                LoopExhaustion::Fail,
            )
            .unwrap();

        let stages = vec![
            make_stage("00_worker", false, false),
            make_stage("01_loop", false, false),
        ];
        let mut plan = ExecutionPlan::from_graph(&stages, graph);

        for _ in 0..2 {
            let next = plan.next_stage().unwrap();
            assert_eq!(next.raw, "00_worker");
            plan.mark_complete(&StageId::parse("00_worker").unwrap());
        }

        // Third attempt should fail because max_iterations=2 and no control
        // outcome resolved the loop.
        assert!(plan.next_stage().is_none());
        assert!(!plan.is_complete());
    }
}
