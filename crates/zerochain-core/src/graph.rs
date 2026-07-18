//! Explicit workflow graph.
//!
//! This module provides a typed graph representation of a zerochain workflow,
//! moving stage-ordering knowledge out of `StageId` string conventions and into
//! explicit nodes and edges. The graph is built from a list of `Stage`s and is
//! consumed by `ExecutionPlan` to decide which stage runs next.
//!
//! Borrowing from the callee loops-and-graphs idea, the graph supports:
//! - `Node::Stage` for normal filesystem-backed stages.
//! - `Node::Loop` for bounded iteration over a sub-graph, with control-record
//!   termination semantics (`Return`, `Escalate`, `Fail`, `Await`).
//!
//! Loops are expanded into the graph during construction so the runtime still
//! sees a flat plan, while loop state is tracked separately by the actor.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::error::{Error, Result};
use crate::stage::{Stage, StageId};

/// Unique handle for a node in the workflow graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub usize);

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Node({})", self.0)
    }
}

/// Result of running a stage or loop body iteration.
///
/// Modeled after callee's control records, but kept as a runtime enum rather
/// than a string protocol so callers cannot emit arbitrary control values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlOutcome {
    /// Default: keep executing the remaining plan.
    Continue,
    /// Stop the enclosing loop and return its natural output.
    Return,
    /// Stop the enclosing loop and escalate to the outer plan.
    Escalate,
    /// Treat the loop as exhausted/failed.
    Fail,
    /// Pause the loop for human approval.
    Await,
}

impl ControlOutcome {
    /// Parse a control record written by a stage into a typed outcome.
    ///
    /// Returns `None` when the record is not a recognized control message,
    /// which is the normal case for stage output.
    #[must_use]
    pub fn parse_record(record: &str) -> Option<Self> {
        match record.trim() {
            "zerochain.control.v1.return" => Some(Self::Return),
            "zerochain.control.v1.escalate" => Some(Self::Escalate),
            "zerochain.control.v1.fail" => Some(Self::Fail),
            "zerochain.control.v1.await" => Some(Self::Await),
            _ => None,
        }
    }

    /// Serialize an outcome into the control-record format.
    #[must_use]
    pub fn as_record(self) -> &'static str {
        match self {
            Self::Continue => "zerochain.control.v1.continue",
            Self::Return => "zerochain.control.v1.return",
            Self::Escalate => "zerochain.control.v1.escalate",
            Self::Fail => "zerochain.control.v1.fail",
            Self::Await => "zerochain.control.v1.await",
        }
    }
}

/// A node in the workflow graph.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Node {
    /// A filesystem-backed stage.
    Stage {
        id: NodeId,
        stage_id: StageId,
        dependencies: Vec<NodeId>,
    },
    /// A bounded loop over a sub-graph.
    ///
    /// The `body` node is the entry of the loop iteration. The loop is
    /// considered exhausted when it completes `max_iterations` without an
    /// early `Return`/`Escalate`/`Fail`/`Await`. The `on_exhausted` flag
    /// decides whether exhaustion is an error.
    Loop {
        id: NodeId,
        stage_id: StageId,
        body: NodeId,
        max_iterations: usize,
        on_exhausted: LoopExhaustion,
        dependencies: Vec<NodeId>,
    },
}

impl Node {
    #[must_use]
    pub fn id(&self) -> NodeId {
        match self {
            Self::Stage { id, .. } | Self::Loop { id, .. } => *id,
        }
    }

    #[must_use]
    pub fn stage_id(&self) -> &StageId {
        match self {
            Self::Stage { stage_id, .. } | Self::Loop { stage_id, .. } => stage_id,
        }
    }

    #[must_use]
    pub fn dependencies(&self) -> &[NodeId] {
        match self {
            Self::Stage { dependencies, .. } | Self::Loop { dependencies, .. } => dependencies,
        }
    }
}

/// What to do when a loop reaches its iteration limit without resolving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopExhaustion {
    /// Exhaustion is an error.
    Fail,
    /// The loop succeeds with the result of the last iteration.
    Succeed,
}

/// An explicit workflow graph.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WorkflowGraph {
    next_id: usize,
    nodes: BTreeMap<NodeId, Node>,
    stage_to_node: HashMap<StageId, NodeId>,
}

impl WorkflowGraph {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: 0,
            nodes: BTreeMap::new(),
            stage_to_node: HashMap::new(),
        }
    }

    fn alloc_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Add a filesystem stage node. The caller is responsible for adding edges
    /// via `add_dependency` or the higher-level builder.
    pub fn add_stage(&mut self, stage_id: StageId) -> NodeId {
        let id = self.alloc_id();
        self.nodes.insert(
            id,
            Node::Stage {
                id,
                stage_id: stage_id.clone(),
                dependencies: Vec::new(),
            },
        );
        self.stage_to_node.insert(stage_id, id);
        id
    }

    /// Add a loop node that wraps an existing body node.
    pub fn add_loop(
        &mut self,
        stage_id: StageId,
        body: NodeId,
        max_iterations: usize,
        on_exhausted: LoopExhaustion,
    ) -> Result<NodeId> {
        if !self.nodes.contains_key(&body) {
            return Err(Error::PlanError {
                reason: format!("loop body node {body} does not exist"),
            });
        }
        let id = self.alloc_id();
        self.nodes.insert(
            id,
            Node::Loop {
                id,
                stage_id: stage_id.clone(),
                body,
                max_iterations,
                on_exhausted,
                dependencies: Vec::new(),
            },
        );
        self.stage_to_node.insert(stage_id, id);
        Ok(id)
    }

    /// Declare that `node` depends on `dependency` completing first.
    pub fn add_dependency(&mut self, node: NodeId, dependency: NodeId) -> Result<()> {
        let n = self.nodes.get_mut(&node).ok_or_else(|| Error::PlanError {
            reason: format!("node {node} does not exist"),
        })?;
        match n {
            Node::Stage { dependencies, .. } | Node::Loop { dependencies, .. } => {
                if !dependencies.contains(&dependency) {
                    dependencies.push(dependency);
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    #[must_use]
    pub fn get_by_stage(&self, stage_id: &StageId) -> Option<&Node> {
        self.stage_to_node
            .get(stage_id)
            .and_then(|id| self.nodes.get(id))
    }

    #[must_use]
    pub fn nodes(&self) -> &BTreeMap<NodeId, Node> {
        &self.nodes
    }

    /// Return the node id associated with a stage id, if any.
    #[must_use]
    pub fn node_id_for_stage(&self, stage_id: &StageId) -> Option<NodeId> {
        self.stage_to_node.get(stage_id).copied()
    }

    /// Return all loop nodes whose body entry is the given stage.
    #[must_use]
    pub fn loops_for_body(&self, body_stage_id: &StageId) -> Vec<NodeId> {
        let Some(body_node_id) = self.node_id_for_stage(body_stage_id) else {
            return Vec::new();
        };
        self.nodes
            .values()
            .filter_map(|n| match n {
                Node::Loop { id, body, .. } if *body == body_node_id => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// Topological order of node ids. Returns an error if a cycle is detected.
    pub fn topological_order(&self) -> Result<Vec<NodeId>> {
        let mut visited = HashSet::new();
        let mut temp = HashSet::new();
        let mut order = Vec::with_capacity(self.nodes.len());

        fn visit(
            graph: &WorkflowGraph,
            id: NodeId,
            visited: &mut HashSet<NodeId>,
            temp: &mut HashSet<NodeId>,
            order: &mut Vec<NodeId>,
        ) -> Result<()> {
            if visited.contains(&id) {
                return Ok(());
            }
            if temp.insert(id) {
                for dep in graph
                    .nodes
                    .get(&id)
                    .map(|n| n.dependencies())
                    .unwrap_or_default()
                {
                    visit(graph, *dep, visited, temp, order)?;
                }
                temp.remove(&id);
                visited.insert(id);
                order.push(id);
                Ok(())
            } else {
                Err(Error::PlanError {
                    reason: format!("cycle detected at node {id}"),
                })
            }
        }

        for id in self.nodes.keys().copied() {
            visit(self, id, &mut visited, &mut temp, &mut order)?;
        }

        Ok(order)
    }
}

/// Builder that creates a `WorkflowGraph` from a list of stages, preserving the
/// existing sequential/parallel semantics derived from `StageId` ordering.
#[derive(Debug, Default)]
pub struct StageGraphBuilder;

impl StageGraphBuilder {
    /// Build a graph from ordered stages.
    ///
    /// Stages are grouped by their numeric sequence. Stages sharing a sequence
    /// (parallel group via letter suffixes like `01a_*`, `01b_*`) form an
    /// independent group with no internal edges. Each group depends on the
    /// previous group, mirroring the old `ExecutionPlan::from_stages` behavior.
    pub fn from_stages(stages: &[Stage]) -> WorkflowGraph {
        let mut graph = WorkflowGraph::new();
        let mut groups: BTreeMap<u32, Vec<StageId>> = BTreeMap::new();

        for stage in stages {
            groups
                .entry(stage.id.sequence)
                .or_default()
                .push(stage.id.clone());
        }

        let mut prev_group_nodes: Vec<NodeId> = Vec::new();

        for (_seq, stage_ids) in groups {
            let mut current_group_nodes = Vec::with_capacity(stage_ids.len());
            for stage_id in stage_ids {
                let node = graph.add_stage(stage_id);
                current_group_nodes.push(node);
            }

            for node in &current_group_nodes {
                for dep in &prev_group_nodes {
                    // Builder cannot fail here because both nodes were just added.
                    let _ = graph.add_dependency(*node, *dep);
                }
            }

            prev_group_nodes = current_group_nodes;
        }

        graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_stage(raw: &str) -> Stage {
        let id = StageId::parse(raw).unwrap();
        Stage {
            id,
            path: PathBuf::from(raw),
            context_path: PathBuf::from(raw).join("CONTEXT.md"),
            input_path: PathBuf::from(raw).join("input"),
            output_path: PathBuf::from(raw).join("output"),
            is_complete: false,
            is_error: false,
            human_gate: false,
            container_image: None,
            command: None,
        }
    }

    #[test]
    fn sequential_dependencies() {
        let stages = vec![
            make_stage("00_spec"),
            make_stage("01_analyze"),
            make_stage("02_implement"),
        ];
        let graph = StageGraphBuilder::from_stages(&stages);
        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 3);

        let spec = graph
            .get_by_stage(&StageId::parse("00_spec").unwrap())
            .unwrap();
        let analyze = graph
            .get_by_stage(&StageId::parse("01_analyze").unwrap())
            .unwrap();
        let implement = graph
            .get_by_stage(&StageId::parse("02_implement").unwrap())
            .unwrap();

        assert!(spec.dependencies().is_empty());
        assert_eq!(analyze.dependencies(), &[spec.id()]);
        assert_eq!(implement.dependencies(), &[analyze.id()]);
    }

    #[test]
    fn parallel_group_no_internal_edges() {
        let stages = vec![
            make_stage("00_spec"),
            make_stage("01a_test_unit"),
            make_stage("01b_test_integration"),
            make_stage("02_deploy"),
        ];
        let graph = StageGraphBuilder::from_stages(&stages);

        let a = graph
            .get_by_stage(&StageId::parse("01a_test_unit").unwrap())
            .unwrap();
        let b = graph
            .get_by_stage(&StageId::parse("01b_test_integration").unwrap())
            .unwrap();
        assert!(!a.dependencies().contains(&b.id()));
        assert!(!b.dependencies().contains(&a.id()));

        let spec = graph
            .get_by_stage(&StageId::parse("00_spec").unwrap())
            .unwrap();
        assert!(a.dependencies().contains(&spec.id()));
        assert!(b.dependencies().contains(&spec.id()));
    }

    #[test]
    fn loop_node_lifecycle() {
        let mut graph = WorkflowGraph::new();
        let body = graph.add_stage(StageId::parse("00_worker").unwrap());
        let stage_id = StageId::parse("01_loop").unwrap();
        let loop_id = graph
            .add_loop(stage_id.clone(), body, 5, LoopExhaustion::Fail)
            .unwrap();

        let node = graph.get(loop_id).unwrap();
        assert_eq!(node.stage_id(), &stage_id);
        assert!(matches!(node, Node::Loop { .. }));
    }

    #[test]
    fn control_outcome_parse_round_trip() {
        assert_eq!(
            ControlOutcome::parse_record("zerochain.control.v1.return"),
            Some(ControlOutcome::Return)
        );
        assert_eq!(
            ControlOutcome::parse_record("zerochain.control.v1.escalate"),
            Some(ControlOutcome::Escalate)
        );
        assert_eq!(ControlOutcome::parse_record("not a control record"), None);
        assert_eq!(
            ControlOutcome::Await.as_record(),
            "zerochain.control.v1.await"
        );
    }
}
