use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use zerochain_memory::{ContributionRecord, ContributionType, GraphView, Verdict};

use crate::handlers::SimpleMessage;
use crate::state::ServerState;

#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    pub view: Option<String>,
    #[serde(rename = "type")]
    pub record_type: Option<String>,
    pub workflow: Option<String>,
    pub tags: Option<String>,
    pub query: Option<String>,
    pub top_k: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ContributionHttpRequest {
    #[serde(rename = "type")]
    pub record_type: String,
    pub body: String,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub metric: Option<HttpMetric>,
    pub workflow: Option<String>,
    pub stage: Option<String>,
    pub actor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HttpMetric {
    pub name: String,
    pub value: f64,
    pub direction: String,
}

#[derive(Debug, Deserialize)]
pub struct VerificationHttpRequest {
    pub target: String,
    pub verdict: String,
    pub body: String,
    pub actor: Option<String>,
    pub workflow: Option<String>,
}

#[allow(clippy::result_large_err)]
async fn open_graph(
    state: &ServerState,
) -> std::result::Result<zerochain_memory::Graph, axum::response::Response> {
    zerochain_memory::Graph::open(state.workspace.join(".zerochain").join("graph"))
        .await
        .map_err(|e| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(SimpleMessage {
                    message: format!("contribution graph unavailable: {e}"),
                }),
            )
                .into_response()
        })
}

pub async fn query(
    State(state): State<ServerState>,
    Query(params): Query<GraphQueryParams>,
) -> impl IntoResponse {
    let graph = match open_graph(&state).await {
        Ok(g) => g,
        Err(resp) => return resp,
    };
    let view = match params.view.as_deref() {
        Some(s) => match GraphView::parse(s) {
            Some(v) => Some(v),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(SimpleMessage {
                        message: format!("unknown view: {s}"),
                    }),
                )
                    .into_response()
            }
        },
        None => None,
    };
    let record_type = match params.record_type.as_deref() {
        Some(s) => match ContributionType::parse(s) {
            Ok(t) => Some(t),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(SimpleMessage {
                        message: e.to_string(),
                    }),
                )
                    .into_response()
            }
        },
        None => None,
    };
    let tags: Vec<String> = params
        .tags
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut records: Vec<ContributionRecord> = graph
        .index()
        .filtered(view, record_type, params.workflow.as_deref(), &tags)
        .into_iter()
        .cloned()
        .collect();

    match params.query.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(q) => {
            let model = match tokio::task::spawn_blocking(zerochain_memory::FastEmbedModel::try_new)
                .await
            {
                Ok(Ok(m)) => m,
                Ok(Err(e)) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(SimpleMessage {
                            message: format!("embedding model: {e}"),
                        }),
                    )
                        .into_response()
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(SimpleMessage {
                            message: e.to_string(),
                        }),
                    )
                        .into_response()
                }
            };
            let cache = state
                .workspace
                .join(".zerochain")
                .join("graph")
                .join("index")
                .join("embeddings.jsonl");
            let embeds =
                match zerochain_memory::GraphEmbedIndex::build(graph.store(), &model, &cache).await
                {
                    Ok(e) => e,
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(SimpleMessage {
                                message: e.to_string(),
                            }),
                        )
                            .into_response()
                    }
                };
            let candidates: std::collections::HashSet<String> =
                records.iter().map(|r| r.id.clone()).collect();
            let top_k = params.top_k.unwrap_or(10) as usize;
            records = match embeds.search(&model, q, Some(&candidates), top_k).await {
                Ok(ranked) => {
                    let mut by_id: std::collections::HashMap<String, ContributionRecord> =
                        records.into_iter().map(|r| (r.id.clone(), r)).collect();
                    ranked
                        .into_iter()
                        .filter_map(|id| by_id.remove(&id))
                        .collect()
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(SimpleMessage {
                            message: e.to_string(),
                        }),
                    )
                        .into_response()
                }
            };
        }
        None => {
            records.sort_by_key(|a| std::cmp::Reverse(a.created));
            records.truncate(params.top_k.unwrap_or(50) as usize);
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "results": records.iter().map(zerochain_memory::record_to_json).collect::<Vec<_>>()
        })),
    )
        .into_response()
}

pub async fn contribute(
    State(state): State<ServerState>,
    Json(body): Json<ContributionHttpRequest>,
) -> impl IntoResponse {
    let record_type = match ContributionType::parse(&body.record_type) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SimpleMessage {
                    message: e.to_string(),
                }),
            )
                .into_response()
        }
    };
    if !matches!(
        record_type,
        ContributionType::Insight | ContributionType::Hypothesis | ContributionType::Report
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(SimpleMessage {
                message: "type must be insight, hypothesis, or report".to_string(),
            }),
        )
            .into_response();
    }
    let mut graph = match open_graph(&state).await {
        Ok(g) => g,
        Err(resp) => return resp,
    };
    let mut record = ContributionRecord::new(
        record_type,
        body.actor.unwrap_or_else(|| "human:api".to_string()),
        body.body,
    );
    record.parents = body.parents;
    record.tags = body.tags;
    record.workflow = body.workflow;
    record.stage = body.stage;
    record.metric = match body.metric {
        Some(m) => match zerochain_memory::MetricDirection::parse(&m.direction) {
            Ok(direction) => Some(zerochain_memory::ContributionMetric {
                name: m.name,
                value: m.value,
                direction,
            }),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(SimpleMessage {
                        message: e.to_string(),
                    }),
                )
                    .into_response()
            }
        },
        None => None,
    };
    match graph.publish(record).await {
        Ok(published) => {
            let id = published.id.clone();
            zerochain_core::jj::auto_commit(
                &state.workspace,
                &format!("graph: {} {id}", record_type.as_str()),
            )
            .await;
            (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(SimpleMessage {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn verify(
    State(state): State<ServerState>,
    Json(body): Json<VerificationHttpRequest>,
) -> impl IntoResponse {
    let verdict = match Verdict::parse(&body.verdict) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SimpleMessage {
                    message: e.to_string(),
                }),
            )
                .into_response()
        }
    };
    let mut graph = match open_graph(&state).await {
        Ok(g) => g,
        Err(resp) => return resp,
    };
    let mut record = ContributionRecord::new(
        ContributionType::Verification,
        body.actor.unwrap_or_else(|| "human:api".to_string()),
        body.body,
    );
    record.target = Some(body.target.clone());
    record.verdict = Some(verdict);
    record.parents = vec![body.target];
    record.workflow = body.workflow;
    match graph.publish(record).await {
        Ok(published) => {
            let id = published.id.clone();
            zerochain_core::jj::auto_commit(&state.workspace, &format!("graph: verification {id}"))
                .await;
            (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(SimpleMessage {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}
