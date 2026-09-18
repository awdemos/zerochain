use async_trait::async_trait;
use serde_json::{json, Value};
use zerochain_error::{Result, ZerochainError};
use zerochain_memory::{
    ContributionMetric, ContributionRecord, ContributionType, Graph, GraphEmbedIndex, GraphView,
    MetricDirection, Verdict,
};

use crate::tool::Tool;

const DEFAULT_TOP_K: usize = 5;

pub struct ContributeTool;

#[async_trait]
impl Tool for ContributeTool {
    fn name(&self) -> &str {
        "contribute"
    }

    fn description(&self) -> &str {
        "Publish a typed contribution (insight, hypothesis, or report) to the workspace contribution graph."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "type": { "type": "string", "enum": ["insight", "hypothesis", "report"], "description": "Contribution type." },
                "body": { "type": "string", "description": "Markdown body of the contribution." },
                "parents": { "type": "array", "items": { "type": "string" }, "description": "Parent contribution IDs. Defaults to the latest contribution in the current workflow." },
                "tags": { "type": "array", "items": { "type": "string" } },
                "metric": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "value": { "type": "number" },
                        "direction": { "type": "string", "enum": ["lower", "higher"] }
                    },
                    "required": ["name", "value", "direction"]
                },
                "graph_dir": { "type": "string", "description": "Injected by the engine; do not set manually." },
                "graph_workflow": { "type": "string", "description": "Injected by the engine; do not set manually." },
                "graph_stage": { "type": "string", "description": "Injected by the engine; do not set manually." },
                "graph_actor": { "type": "string", "description": "Injected by the engine; do not set manually." }
            },
            "required": ["type", "body"]
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let record_type = ContributionType::parse(required_str(&input, "type")?).map_err(|_| {
            ZerochainError::InvalidInput {
                message: "type must be insight, hypothesis, or report".to_string(),
            }
        })?;
        if !matches!(
            record_type,
            ContributionType::Insight | ContributionType::Hypothesis | ContributionType::Report
        ) {
            return Err(ZerochainError::InvalidInput {
                message: "contribute type must be insight, hypothesis, or report".to_string(),
            });
        }
        let body = required_str(&input, "body")?.to_string();
        let graph_dir = required_str(&input, "graph_dir")?;
        let workflow = optional_str(&input, "graph_workflow");
        let actor =
            optional_str(&input, "graph_actor").unwrap_or_else(|| "zerochain/unknown".into());

        let mut graph = Graph::open(graph_dir).await?;
        let parents = match input.get("parents").and_then(Value::as_array) {
            Some(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            None => workflow
                .as_deref()
                .and_then(|wf| {
                    graph
                        .index()
                        .latest_in_workflow(wf)
                        .map(|r| vec![r.id.clone()])
                })
                .unwrap_or_default(),
        };
        let mut record = ContributionRecord::new(record_type, actor, body);
        record.parents = parents;
        record.workflow = workflow;
        record.stage = optional_str(&input, "graph_stage");
        record.metric = parse_metric(input.get("metric"))?;
        record.tags = string_array(input.get("tags"));
        let published = graph.publish(record).await?;
        Ok(json!({ "id": published.id }))
    }
}

pub struct VerifyTool;

#[async_trait]
impl Tool for VerifyTool {
    fn name(&self) -> &str {
        "verify"
    }

    fn description(&self) -> &str {
        "Publish a verification verdict (confirmed, partial, or failed) for a contribution in the workspace graph."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": { "type": "string", "description": "Contribution ID to verify." },
                "verdict": { "type": "string", "enum": ["confirmed", "partial", "failed"] },
                "body": { "type": "string", "description": "Evidence for the verdict." },
                "graph_dir": { "type": "string", "description": "Injected by the engine; do not set manually." },
                "graph_workflow": { "type": "string", "description": "Injected by the engine; do not set manually." },
                "graph_actor": { "type": "string", "description": "Injected by the engine; do not set manually." }
            },
            "required": ["target", "verdict", "body"]
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let target = required_str(&input, "target")?.to_string();
        let verdict = Verdict::parse(required_str(&input, "verdict")?).map_err(|e| {
            ZerochainError::InvalidInput {
                message: e.to_string(),
            }
        })?;
        let body = required_str(&input, "body")?.to_string();
        let graph_dir = required_str(&input, "graph_dir")?;
        let actor =
            optional_str(&input, "graph_actor").unwrap_or_else(|| "zerochain/unknown".into());

        let mut graph = Graph::open(graph_dir).await?;
        let mut record = ContributionRecord::new(ContributionType::Verification, actor, body);
        record.target = Some(target.clone());
        record.verdict = Some(verdict);
        record.parents = vec![target];
        record.workflow = optional_str(&input, "graph_workflow");
        let published = graph.publish(record).await?;
        Ok(json!({ "id": published.id }))
    }
}

pub struct GraphQueryTool;

#[async_trait]
impl Tool for GraphQueryTool {
    fn name(&self) -> &str {
        "graph_query"
    }

    fn description(&self) -> &str {
        "Search the workspace contribution graph semantically, or list a named view (recent, leaves, open_hypotheses, unverified, negative, leaders)."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Semantic search text over contribution bodies." },
                "view": { "type": "string", "enum": ["recent", "leaves", "open_hypotheses", "unverified", "negative", "leaders"] },
                "type": { "type": "string", "description": "Filter by contribution type." },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "Filter: records carrying all listed tags." },
                "workflow": { "type": "string", "description": "Filter by workflow." },
                "top_k": { "type": "number", "description": "Max results (default 5)." },
                "graph_dir": { "type": "string", "description": "Injected by the engine; do not set manually." }
            }
        })
    }

    async fn run(&self, input: Value) -> Result<Value> {
        let graph_dir = required_str(&input, "graph_dir")?;
        let top_k = input
            .get("top_k")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_TOP_K);
        let graph = Graph::open(graph_dir).await?;

        let view = match input.get("view").and_then(Value::as_str) {
            Some(s) => Some(
                GraphView::parse(s).ok_or_else(|| ZerochainError::InvalidInput {
                    message: format!("unknown view: {s}"),
                })?,
            ),
            None => None,
        };
        let record_type = input
            .get("type")
            .and_then(Value::as_str)
            .map(ContributionType::parse)
            .transpose()
            .map_err(|e| ZerochainError::InvalidInput {
                message: e.to_string(),
            })?;
        let workflow = optional_str(&input, "workflow");
        let tags = string_array(input.get("tags"));

        let mut records: Vec<ContributionRecord> = match view {
            Some(v) => graph.index().view(v).into_iter().cloned().collect(),
            None => graph.index().all().into_iter().cloned().collect(),
        };
        if let Some(t) = record_type {
            records.retain(|r| r.record_type == t);
        }
        if let Some(wf) = &workflow {
            records.retain(|r| r.workflow.as_deref() == Some(wf.as_str()));
        }
        if !tags.is_empty() {
            records.retain(|r| tags.iter().all(|t| r.tags.contains(t)));
        }

        let ordered: Vec<ContributionRecord> = match input
            .get("query")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            Some(query) => {
                let model = tokio::task::spawn_blocking(zerochain_memory::FastEmbedModel::try_new)
                    .await
                    .map_err(|e| ZerochainError::Other {
                        message: e.to_string(),
                    })?
                    .map_err(|e| ZerochainError::Other {
                        message: format!("failed to initialize embedding model: {e}"),
                    })?;
                let cache = std::path::Path::new(graph_dir)
                    .join("index")
                    .join("embeddings.jsonl");
                let embeds = GraphEmbedIndex::build(graph.store(), &model, &cache).await?;
                let candidates: std::collections::HashSet<String> =
                    records.iter().map(|r| r.id.clone()).collect();
                let ranked = embeds
                    .search(&model, query, Some(&candidates), top_k)
                    .await?;
                let mut by_id: std::collections::HashMap<String, ContributionRecord> =
                    records.into_iter().map(|r| (r.id.clone(), r)).collect();
                ranked
                    .into_iter()
                    .filter_map(|id| by_id.remove(&id))
                    .collect()
            }
            None => {
                records.sort_by_key(|a| std::cmp::Reverse(a.created));
                records.truncate(top_k);
                records
            }
        };

        let results: Vec<Value> = ordered.iter().map(record_json).collect();
        Ok(json!({ "results": results }))
    }
}

fn record_json(r: &ContributionRecord) -> Value {
    json!({
        "id": r.id,
        "type": r.record_type.as_str(),
        "parents": r.parents,
        "actor": r.actor,
        "created": r.created.to_rfc3339(),
        "workflow": r.workflow,
        "metric": r.metric.as_ref().map(|m| {
            json!({
                "name": m.name,
                "value": m.value,
                "direction": serde_json::to_value(m.direction).unwrap_or(Value::Null),
            })
        }),
        "excerpt": excerpt(&r.body),
    })
}

fn excerpt(body: &str) -> String {
    let flat: String = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join(" ");
    flat.chars().take(240).collect()
}

fn required_str<'v>(input: &'v Value, key: &str) -> Result<&'v str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ZerochainError::InvalidInput {
            message: format!("missing '{key}' field"),
        })
}

fn optional_str(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::to_string)
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_metric(value: Option<&Value>) -> Result<Option<ContributionMetric>> {
    let Some(v) = value else {
        return Ok(None);
    };
    let name =
        v.get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ZerochainError::InvalidInput {
                message: "metric requires 'name'".to_string(),
            })?;
    let value =
        v.get("value")
            .and_then(Value::as_f64)
            .ok_or_else(|| ZerochainError::InvalidInput {
                message: "metric requires numeric 'value'".to_string(),
            })?;
    let direction =
        MetricDirection::parse(v.get("direction").and_then(Value::as_str).ok_or_else(|| {
            ZerochainError::InvalidInput {
                message: "metric requires 'direction'".to_string(),
            }
        })?)
        .map_err(|e| ZerochainError::InvalidInput {
            message: e.to_string(),
        })?;
    Ok(Some(ContributionMetric {
        name: name.to_string(),
        value,
        direction,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn base_input(tmp: &TempDir) -> Value {
        json!({
            "graph_dir": tmp.path().join("graph").to_str().unwrap(),
            "graph_workflow": "wf",
            "graph_actor": "zerochain/test"
        })
    }

    #[tokio::test]
    async fn contribute_publishes_with_default_parent() {
        let tmp = TempDir::new().unwrap();
        let mut graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let mut setup_rec = ContributionRecord::new(ContributionType::Setup, "a", "root");
        setup_rec.workflow = Some("wf".to_string());
        let setup = graph.publish(setup_rec).await.unwrap();

        let tool = ContributeTool;
        let mut input = base_input(&tmp);
        input["type"] = json!("insight");
        input["body"] = json!("donor ensembling helps");
        let result = tool.run(input).await.unwrap();
        let id = result
            .get("id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();

        let graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let rec = graph.index().get(&id).unwrap().clone();
        assert_eq!(rec.parents, vec![setup.id]);
        assert_eq!(rec.workflow.as_deref(), Some("wf"));
    }

    #[tokio::test]
    async fn contribute_rejects_result_type() {
        let tmp = TempDir::new().unwrap();
        let tool = ContributeTool;
        let mut input = base_input(&tmp);
        input["type"] = json!("result");
        input["body"] = json!("x");
        assert!(tool.run(input).await.is_err());
    }

    #[tokio::test]
    async fn verify_publishes_and_rejects_bad_target() {
        let tmp = TempDir::new().unwrap();
        let mut graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let target = graph
            .publish(ContributionRecord::new(
                ContributionType::Result,
                "a",
                "measured",
            ))
            .await
            .unwrap()
            .id;

        let tool = VerifyTool;
        let mut input = base_input(&tmp);
        input["target"] = json!(target);
        input["verdict"] = json!("confirmed");
        input["body"] = json!("bit-identical reproduction");
        let result = tool.run(input).await.unwrap();
        let vid = result
            .get("id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();

        let graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let v = graph.index().get(&vid).unwrap().clone();
        assert_eq!(v.record_type, ContributionType::Verification);
        assert_eq!(v.parents, vec![target.clone()]);

        let mut bad = base_input(&tmp);
        bad["target"] = json!("c-missing00000000");
        bad["verdict"] = json!("confirmed");
        bad["body"] = json!("x");
        assert!(tool.run(bad).await.is_err());
    }

    #[tokio::test]
    async fn graph_query_lists_views_without_embeddings() {
        let tmp = TempDir::new().unwrap();
        let mut graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let setup = graph
            .publish(ContributionRecord::new(
                ContributionType::Setup,
                "a",
                "root",
            ))
            .await
            .unwrap();
        let mut result = ContributionRecord::new(ContributionType::Result, "a", "measured thing");
        result.parents = vec![setup.id.clone()];
        result.workflow = Some("wf".to_string());
        result.metric = Some(ContributionMetric {
            name: "bpb".into(),
            value: 1.9,
            direction: MetricDirection::Lower,
        });
        graph.publish(result).await.unwrap();

        let tool = GraphQueryTool;
        let mut input = base_input(&tmp);
        input["view"] = json!("leaders");
        let out = tool.run(input).await.unwrap();
        let results = out.get("results").and_then(Value::as_array).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].get("type").and_then(Value::as_str),
            Some("result")
        );
    }
}
