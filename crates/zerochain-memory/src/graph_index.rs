use std::collections::{BTreeMap, HashMap};

use crate::record::{ContributionRecord, ContributionType, Verdict};

/// Named views over the graph (spec §4.2, §6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphView {
    Recent,
    Leaves,
    OpenHypotheses,
    Unverified,
    Negative,
    Leaders,
}

impl GraphView {
    pub fn as_str(&self) -> &'static str {
        match self {
            GraphView::Recent => "recent",
            GraphView::Leaves => "leaves",
            GraphView::OpenHypotheses => "open_hypotheses",
            GraphView::Unverified => "unverified",
            GraphView::Negative => "negative",
            GraphView::Leaders => "leaders",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "recent" => Some(GraphView::Recent),
            "leaves" => Some(GraphView::Leaves),
            "open_hypotheses" => Some(GraphView::OpenHypotheses),
            "unverified" => Some(GraphView::Unverified),
            "negative" => Some(GraphView::Negative),
            "leaders" => Some(GraphView::Leaders),
            _ => None,
        }
    }
}

/// Derived, rebuildable index over contribution records.
#[derive(Debug, Default)]
pub struct GraphIndex {
    records: BTreeMap<String, ContributionRecord>,
    children: HashMap<String, Vec<String>>,
}

impl GraphIndex {
    pub fn from_records(records: Vec<ContributionRecord>) -> Self {
        let mut index = GraphIndex::default();
        for record in records {
            index.add(record);
        }
        index
    }

    pub fn add(&mut self, record: ContributionRecord) {
        for parent in &record.parents {
            self.children
                .entry(parent.clone())
                .or_default()
                .push(record.id.clone());
        }
        self.records.insert(record.id.clone(), record);
    }

    pub fn get(&self, id: &str) -> Option<&ContributionRecord> {
        self.records.get(id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// All records, oldest first.
    pub fn all(&self) -> Vec<&ContributionRecord> {
        let mut recs: Vec<&ContributionRecord> = self.records.values().collect();
        recs.sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
        recs
    }

    /// Most recent contribution published in the given workflow — the parent
    /// chaining rule for auto-captured `result` nodes (spec §5).
    pub fn latest_in_workflow(&self, workflow: &str) -> Option<&ContributionRecord> {
        self.records
            .values()
            .filter(|r| r.workflow.as_deref() == Some(workflow))
            .max_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)))
    }

    /// Effective verdicts for a target: newest verification per actor.
    /// Replaceable-verdict semantics (spec §3): both records stay in the DAG;
    /// only the effect is superseded.
    pub fn effective_verdicts(&self, target: &str) -> Vec<(String, Verdict)> {
        let mut per_actor: HashMap<String, (chrono::DateTime<chrono::Utc>, Verdict)> =
            HashMap::new();
        for record in self.records.values() {
            if record.record_type != ContributionType::Verification {
                continue;
            }
            if record.target.as_deref() != Some(target) {
                continue;
            }
            let Some(actor) = (!record.actor.is_empty()).then(|| record.actor.clone()) else {
                continue;
            };
            let verdict = record.verdict.unwrap_or(Verdict::Failed);
            match per_actor.get(&actor) {
                Some((created, _)) if *created >= record.created => {}
                _ => {
                    per_actor.insert(actor, (record.created, verdict));
                }
            }
        }
        per_actor.into_iter().map(|(a, (_, v))| (a, v)).collect()
    }

    fn is_verified(&self, record: &ContributionRecord) -> bool {
        self.effective_verdicts(&record.id)
            .iter()
            .any(|(_, v)| matches!(v, Verdict::Confirmed | Verdict::Partial))
    }

    /// Records matching a named view.
    pub fn view(&self, view: GraphView) -> Vec<&ContributionRecord> {
        match view {
            GraphView::Recent => {
                let mut recs = self.all();
                recs.reverse();
                recs
            }
            // Frontier: childless records that can still be built on.
            // Verifications are never parents for new work, so exclude them.
            GraphView::Leaves => self
                .records
                .values()
                .filter(|r| {
                    r.record_type != ContributionType::Verification
                        && !self.children.contains_key(&r.id)
                })
                .collect(),
            GraphView::OpenHypotheses => self
                .records
                .values()
                .filter(|r| r.record_type == ContributionType::Hypothesis)
                .collect(),
            GraphView::Negative => self
                .records
                .values()
                .filter(|r| r.tags.iter().any(|t| t == "negative"))
                .collect(),
            // Results with no effective confirmed/partial verification from
            // any actor (spec §4.2).
            GraphView::Unverified => self
                .records
                .values()
                .filter(|r| r.record_type == ContributionType::Result && !self.is_verified(r))
                .collect(),
            // Best results per metric group, respecting direction.
            GraphView::Leaders => {
                let mut groups: HashMap<
                    (String, crate::record::MetricDirection),
                    Vec<&ContributionRecord>,
                > = HashMap::new();
                for record in self.records.values() {
                    if record.record_type != ContributionType::Result {
                        continue;
                    }
                    let Some(metric) = &record.metric else {
                        continue;
                    };
                    groups
                        .entry((metric.name.clone(), metric.direction))
                        .or_default()
                        .push(record);
                }
                let mut leaders = Vec::new();
                for ((_, direction), mut group) in groups {
                    group.sort_by(|a, b| {
                        let ord = a
                            .metric
                            .as_ref()
                            .unwrap()
                            .value
                            .partial_cmp(&b.metric.as_ref().unwrap().value)
                            .unwrap_or(std::cmp::Ordering::Equal);
                        match direction {
                            crate::record::MetricDirection::Lower => ord,
                            crate::record::MetricDirection::Higher => ord.reverse(),
                        }
                    });
                    leaders.push(group[0]);
                }
                leaders.sort_by(|a, b| a.id.cmp(&b.id));
                leaders
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{ContributionMetric, ContributionType, MetricDirection, Verdict};
    use chrono::Duration;

    struct GraphBuilder {
        index: GraphIndex,
        n: usize,
        workflow: String,
    }

    impl GraphBuilder {
        fn new(workflow: &str) -> Self {
            GraphBuilder {
                index: GraphIndex::default(),
                n: 0,
                workflow: workflow.to_string(),
            }
        }

        fn push(
            &mut self,
            record_type: ContributionType,
            actor: &str,
            body: &str,
            parents: Vec<String>,
        ) -> String {
            self.n += 1;
            let mut rec = ContributionRecord::new(record_type, actor, body);
            rec.created = chrono::Utc::now() + Duration::milliseconds(self.n as i64);
            rec.parents = parents;
            rec.workflow = Some(self.workflow.clone());
            let id = rec.compute_id();
            rec.id = id.clone();
            self.index.add(rec);
            id
        }

        fn verify(&mut self, actor: &str, target: &str, verdict: Verdict, body: &str) -> String {
            self.n += 1;
            let mut rec = ContributionRecord::new(ContributionType::Verification, actor, body);
            rec.created = chrono::Utc::now() + Duration::milliseconds(self.n as i64);
            rec.target = Some(target.to_string());
            rec.verdict = Some(verdict);
            rec.parents = vec![target.to_string()];
            rec.workflow = Some(self.workflow.clone());
            let id = rec.compute_id();
            rec.id = id.clone();
            self.index.add(rec);
            id
        }
    }

    #[test]
    fn leaves_exclude_verifications_and_parented_nodes() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let result = g.push(ContributionType::Result, "a", "r1", vec![setup.clone()]);
        let _v = g.verify("b", &result, Verdict::Confirmed, "reproduced");
        let leaves = g.index.view(GraphView::Leaves);
        let ids: Vec<&str> = leaves.iter().map(|r| r.id.as_str()).collect();
        assert!(!ids.contains(&setup.as_str()), "setup has a child");
        assert!(
            !ids.contains(&result.as_str()),
            "result has a verification child"
        );
        assert_eq!(
            leaves.len(),
            0,
            "setup and result both have children; verification excluded"
        );
    }

    #[test]
    fn leaves_include_childless_results() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let r1 = g.push(ContributionType::Result, "a", "r1", vec![setup.clone()]);
        let _r2 = g.push(ContributionType::Result, "a", "r2", vec![r1.clone()]);
        let leaves = g.index.view(GraphView::Leaves);
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].body, "r2");
    }

    #[test]
    fn unverified_respects_failed_and_confirmed_verdicts() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let r1 = g.push(ContributionType::Result, "a", "r1", vec![setup.clone()]);
        let r2 = g.push(ContributionType::Result, "a", "r2", vec![setup.clone()]);
        g.verify("b", &r1, Verdict::Failed, "did not reproduce");
        let unverified: Vec<&str> = g
            .index
            .view(GraphView::Unverified)
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert!(
            unverified.contains(&r1.as_str()),
            "failed verdict leaves result unverified"
        );
        assert!(unverified.contains(&r2.as_str()));

        g.verify("c", &r1, Verdict::Confirmed, "reproduced on H100");
        let unverified: Vec<&str> = g
            .index
            .view(GraphView::Unverified)
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert!(!unverified.contains(&r1.as_str()));
    }

    #[test]
    fn newer_verdict_supersedes_older_per_actor() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let r1 = g.push(ContributionType::Result, "a", "r1", vec![setup.clone()]);
        g.verify("b", &r1, Verdict::Confirmed, "first pass");
        g.verify("b", &r1, Verdict::Failed, "second pass failed");
        let verdicts = g.index.effective_verdicts(&r1);
        assert_eq!(verdicts.len(), 1, "one actor -> one effective verdict");
        assert_eq!(verdicts[0].1, Verdict::Failed, "newest verdict wins");
    }

    #[test]
    fn leaders_group_by_metric_and_direction() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let mut r1 = ContributionRecord::new(ContributionType::Result, "a", "good");
        r1.parents = vec![setup.clone()];
        r1.workflow = Some("wf".to_string());
        r1.metric = Some(ContributionMetric {
            name: "bpb".to_string(),
            value: 1.9,
            direction: MetricDirection::Lower,
        });
        r1.created = chrono::Utc::now();
        let r1_id = r1.compute_id();
        r1.id = r1_id.clone();
        g.index.add(r1);

        let mut r2 = ContributionRecord::new(ContributionType::Result, "a", "worse");
        r2.parents = vec![r1_id.clone()];
        r2.workflow = Some("wf".to_string());
        r2.metric = Some(ContributionMetric {
            name: "bpb".to_string(),
            value: 2.5,
            direction: MetricDirection::Lower,
        });
        r2.created = chrono::Utc::now();
        let r2_id = r2.compute_id();
        r2.id = r2_id.clone();
        g.index.add(r2);

        let leaders = g.index.view(GraphView::Leaders);
        assert_eq!(leaders.len(), 1, "one metric group");
        assert_eq!(
            leaders[0].metric.as_ref().unwrap().value,
            1.9,
            "lower is better"
        );
    }

    #[test]
    fn latest_in_workflow_picks_most_recent() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        let r1 = g.push(ContributionType::Result, "a", "r1", vec![setup.clone()]);
        let latest = g.index.latest_in_workflow("wf").unwrap();
        assert_eq!(latest.id, r1);
        assert!(g.index.latest_in_workflow("other").is_none());
    }

    #[test]
    fn open_hypotheses_and_negative_views() {
        let mut g = GraphBuilder::new("wf");
        let setup = g.push(ContributionType::Setup, "a", "root", vec![]);
        g.push(ContributionType::Hypothesis, "a", "h1", vec![setup.clone()]);
        let mut neg = ContributionRecord::new(ContributionType::Result, "a", "flopped");
        neg.parents = vec![setup.clone()];
        neg.tags = vec!["negative".to_string()];
        neg.created = chrono::Utc::now();
        neg.workflow = Some("wf".to_string());
        let neg_id = neg.compute_id();
        neg.id = neg_id.clone();
        g.index.add(neg);

        assert_eq!(g.index.view(GraphView::OpenHypotheses).len(), 1);
        let negative = g.index.view(GraphView::Negative);
        assert_eq!(negative.len(), 1);
        assert_eq!(negative[0].body, "flopped");
    }
}
