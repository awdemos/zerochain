use anyhow::Result;
use clap::Parser;
use zerochain_core::stage::StageId;
use zerochain_core::template::TemplateRegistry;
use zerochain_engine::AppState;

fn human_actor() -> String {
    let id = std::env::var("ZEROCHAIN_OKF_ACTOR")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_string());
    format!("human:{id}")
}

fn parse_metric_spec(spec: &str) -> anyhow::Result<zerochain_memory::ContributionMetric> {
    let mut name = None;
    let mut value = None;
    let mut direction = None;
    for part in spec.split(',') {
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("metric spec must be key=value pairs: {spec}"))?;
        match k.trim() {
            "name" => name = Some(v.trim().to_string()),
            "value" => {
                let parsed: f64 = v
                    .trim()
                    .parse()
                    .map_err(|e| anyhow::anyhow!("metric value: {e}"))?;
                if !parsed.is_finite() {
                    return Err(anyhow::anyhow!("metric value must be finite"));
                }
                value = Some(parsed);
            }
            "direction" => direction = Some(v.trim().to_string()),
            other => return Err(anyhow::anyhow!("unknown metric key: {other}")),
        }
    }
    Ok(zerochain_memory::ContributionMetric {
        name: name.ok_or_else(|| anyhow::anyhow!("metric requires name"))?,
        value: value.ok_or_else(|| anyhow::anyhow!("metric requires value"))?,
        direction: zerochain_memory::MetricDirection::parse(
            &direction.ok_or_else(|| anyhow::anyhow!("metric requires direction"))?,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let cli = zerochain_daemon::cli::Cli::parse();
    if let Ok(env_workspace) = std::env::var("ZEROCHAIN_WORKSPACE") {
        let env_path = std::path::PathBuf::from(&env_workspace);
        if cli.workspace != env_path {
            return Err(anyhow::anyhow!(
                "workspace conflict: ZEROCHAIN_WORKSPACE is set to '{}' but --workspace is '{}'; unset the environment variable or omit --workspace to use the same path",
                env_workspace,
                cli.workspace.display()
            ));
        }
    }
    let mut state = AppState::new(&cli.workspace, None).await;
    state.load_workflows().await?;

    match cli.command {
        zerochain_daemon::cli::Commands::Init {
            name,
            path,
            template,
            force,
            parents,
        } => {
            // Ensure a jj repo exists so engine auto-commit finds one.
            zerochain_core::jj::init_repo(path.as_deref().unwrap_or(&cli.workspace)).await;
            state
                .init_workflow(zerochain_engine::InitWorkflowParams {
                    name: &name,
                    path: path.as_deref(),
                    template: template.as_deref(),
                    force,
                    parents,
                })
                .await?;
            println!("initialized workflow: {name}");
        }
        zerochain_daemon::cli::Commands::Run { workflow_id, stage } => {
            let workflow = state
                .get_workflow(&workflow_id)
                .ok_or_else(|| anyhow::anyhow!("workflow not found: {workflow_id}"))?;
            let plan = workflow.execution_plan();

            if plan.is_complete() {
                println!("workflow complete: {workflow_id}");
                return Ok(());
            }

            let stage_id = if let Some(s) = &stage {
                StageId::parse(s).map_err(|e| anyhow::anyhow!("{e}"))?
            } else {
                let next = plan
                    .next_stage()
                    .ok_or_else(|| anyhow::anyhow!("no pending stages"))?;
                next.clone()
            };

            let stage = workflow
                .stage_by_id(&stage_id)
                .ok_or_else(|| anyhow::anyhow!("stage not found: {}", stage_id.raw))?
                .clone();

            println!("executing stage {} in {}", stage_id.raw, workflow_id);
            println!("  input:  {}", stage.input_path.display());
            println!("  output: {}", stage.output_path.display());

            state.run_stage(&workflow_id, &stage_id.raw).await?;

            println!("stage complete: {}", stage_id.raw);
        }
        zerochain_daemon::cli::Commands::Status { workflow_id: None } => {
            let workflows = state.list_workflows();
            if workflows.is_empty() {
                println!("no workflows");
                return Ok(());
            }
            for (id, status) in workflows {
                println!("{id}\t{status}");
            }
        }
        zerochain_daemon::cli::Commands::Status {
            workflow_id: Some(wid),
        } => {
            let workflow = state
                .get_workflow(&wid)
                .ok_or_else(|| anyhow::anyhow!("workflow not found: {wid}"))?;
            let plan = workflow.execution_plan();
            let complete = plan.is_complete();
            let next = plan.next_stage().map_or("none", |s| s.raw.as_str());
            println!("id:       {}", workflow.id);
            println!("root:     {}", workflow.root.display());
            println!("stages:   {}", workflow.stages.len());
            println!("complete: {complete}");
            println!("next:     {next}");
            for stage in &workflow.stages {
                let marker = if stage.is_complete {
                    "done"
                } else if stage.is_error {
                    "error"
                } else if stage.human_gate {
                    "gate"
                } else {
                    "pending"
                };
                println!("  {} [{}]", stage.id.raw, marker);
            }
        }
        zerochain_daemon::cli::Commands::List => {
            let workflows = state.list_workflows();
            if workflows.is_empty() {
                println!("no workflows");
                return Ok(());
            }
            for (id, status) in workflows {
                println!("{id}\t{status}");
            }
        }
        zerochain_daemon::cli::Commands::Approve {
            workflow_id,
            stage_id,
        } => {
            state
                .mark_stage_complete(&workflow_id, &stage_id, None)
                .await?;
            println!("approved: {workflow_id} / {stage_id}");
        }
        zerochain_daemon::cli::Commands::Reject {
            workflow_id,
            stage_id,
            feedback,
        } => {
            state
                .mark_stage_error(&workflow_id, &stage_id, feedback.as_deref())
                .await?;
            println!("rejected: {workflow_id} / {stage_id}");
        }
        zerochain_daemon::cli::Commands::Templates => {
            let registry = TemplateRegistry::new();
            let list = registry.list();
            if list.is_empty() {
                println!("no templates available");
                return Ok(());
            }
            for template in list {
                println!("{}\t{}", template.name, template.description);
                for stage in &template.stages {
                    let gate = if stage.human_gate { " [gate]" } else { "" };
                    println!("  {} {}{}", stage.name, stage.role, gate);
                }
            }
        }
        zerochain_daemon::cli::Commands::ExportOkf {
            workflow_id,
            output,
        } => {
            let output_dir =
                output.unwrap_or_else(|| std::path::PathBuf::from(format!("{}-okf", workflow_id)));
            state.export_okf(&workflow_id, &output_dir).await?;
            println!("exported OKF bundle: {}", output_dir.display());
        }
        zerochain_daemon::cli::Commands::Contribute {
            kind,
            body,
            parents,
            tags,
            metric,
        } => {
            let record_type = zerochain_memory::ContributionType::parse(&kind)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if !matches!(
                record_type,
                zerochain_memory::ContributionType::Insight
                    | zerochain_memory::ContributionType::Hypothesis
                    | zerochain_memory::ContributionType::Report
            ) {
                return Err(anyhow::anyhow!(
                    "contribute type must be insight, hypothesis, or report (use the verify command for verdicts)"
                ));
            }
            let graph_dir = cli.workspace.join(".zerochain").join("graph");
            let mut graph = zerochain_memory::Graph::open(&graph_dir).await?;
            let mut record =
                zerochain_memory::ContributionRecord::new(record_type, human_actor(), body);
            record.parents = parents;
            record.metric = metric.as_deref().map(parse_metric_spec).transpose()?;
            record.tags = tags;
            let published = graph.publish(record).await?;
            zerochain_core::jj::auto_commit(
                &cli.workspace,
                &format!("graph: {} {}", record_type.as_str(), published.id),
            )
            .await;
            println!("published contribution: {}", published.id);
        }
        zerochain_daemon::cli::Commands::Verify {
            target,
            verdict,
            body,
        } => {
            let verdict =
                zerochain_memory::Verdict::parse(&verdict).map_err(|e| anyhow::anyhow!("{e}"))?;
            let graph_dir = cli.workspace.join(".zerochain").join("graph");
            let mut graph = zerochain_memory::Graph::open(&graph_dir).await?;
            let mut record = zerochain_memory::ContributionRecord::new(
                zerochain_memory::ContributionType::Verification,
                human_actor(),
                body,
            );
            record.target = Some(target.clone());
            record.verdict = Some(verdict);
            record.parents = vec![target];
            let published = graph.publish(record).await?;
            zerochain_core::jj::auto_commit(
                &cli.workspace,
                &format!("graph: verification {}", published.id),
            )
            .await;
            println!("published verification: {}", published.id);
        }
        zerochain_daemon::cli::Commands::Graph {
            view,
            record_type,
            workflow,
            json,
        } => {
            let graph_dir = cli.workspace.join(".zerochain").join("graph");
            let graph = zerochain_memory::Graph::open(&graph_dir).await?;
            let view = view
                .as_deref()
                .map(|s| {
                    zerochain_memory::GraphView::parse(s)
                        .ok_or_else(|| anyhow::anyhow!("unknown view: {s}"))
                })
                .transpose()?;
            let record_type = record_type
                .as_deref()
                .map(|s| {
                    zerochain_memory::ContributionType::parse(s).map_err(|e| anyhow::anyhow!("{e}"))
                })
                .transpose()?;
            let mut records: Vec<zerochain_memory::ContributionRecord> = match view {
                Some(v) => graph.index().view(v).into_iter().cloned().collect(),
                None => graph.index().all().into_iter().cloned().collect(),
            };
            if let Some(t) = record_type {
                records.retain(|r| r.record_type == t);
            }
            if let Some(wf) = &workflow {
                records.retain(|r| r.workflow.as_deref() == Some(wf.as_str()));
            }
            if json {
                let out: Vec<serde_json::Value> = records
                    .iter()
                    .map(zerochain_memory::record_to_json)
                    .collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                for r in &records {
                    let first_line = r
                        .body
                        .lines()
                        .map(str::trim)
                        .find(|l| !l.is_empty())
                        .unwrap_or("");
                    println!(
                        "{}\t{}\t{}\t{}",
                        r.id,
                        r.record_type.as_str(),
                        r.actor,
                        first_line
                    );
                }
            }
        }
        zerochain_daemon::cli::Commands::Mcp => {
            zerochain_daemon::mcp::run_stdio_server(cli.workspace).await?;
        }
    }

    Ok(())
}
