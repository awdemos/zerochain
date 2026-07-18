use anyhow::Result;
use clap::Parser;
use zerochain_core::stage::StageId;
use zerochain_core::template::TemplateRegistry;
use zerochain_engine::AppState;

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
        } => {
            state
                .init_workflow(zerochain_engine::InitWorkflowParams {
                    name: &name,
                    path: path.as_deref(),
                    template: template.as_deref(),
                    force,
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
        zerochain_daemon::cli::Commands::Mcp => {
            zerochain_daemon::mcp::run_stdio_server(cli.workspace).await?;
        }
    }

    Ok(())
}
