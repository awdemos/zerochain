use anyhow::Result;
use clap::{Parser, Subcommand};
use zerochain_daemon::state::AppState;
use std::path::PathBuf;
use zerochain_fs::{acquire_lock, clean_output, mark_complete};
use zerochain_core::stage::StageId;

#[derive(Parser)]
#[command(
    name = "zerochain",
    version,
    about = "Filesystem-native workflow engine — build AI agents with mkdir",
    after_long_help = "Stage config: CONTEXT.md (YAML) or CONTEXT.lua (Lua script)\n\
                       Docs: https://github.com/awdemos/zerochain"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, env = "ZEROCHAIN_WORKSPACE", default_value = "./workspace")]
    workspace: PathBuf,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Create a new workflow with numbered stages")]
    Init {
        #[arg(short, long, help = "Workflow name")]
        name: String,
        #[arg(short, long, help = "Path to workspace root")]
        path: Option<PathBuf>,
        #[arg(short, long, help = "Comma-separated stage names (e.g. \"research,design,implement\")")]
        template: Option<String>,
    },
    #[command(about = "Execute the next pending stage (or a specific stage)")]
    Run {
        #[arg(help = "Workflow ID")]
        workflow_id: String,
        #[arg(short, long, help = "Specific stage to run (e.g. 02_design)")]
        stage: Option<String>,
    },
    #[command(about = "Show workflow status and stage states")]
    Status {
        #[arg(help = "Workflow ID (omit to list all)")]
        workflow_id: Option<String>,
    },
    #[command(about = "List all workflows")]
    List,
    #[command(about = "Approve a stage waiting at a human gate")]
    Approve {
        #[arg(help = "Workflow ID")]
        workflow_id: String,
        #[arg(help = "Stage ID (e.g. 03_review)")]
        stage_id: String,
    },
    #[command(about = "Reject a stage and mark it as error")]
    Reject {
        #[arg(help = "Workflow ID")]
        workflow_id: String,
        #[arg(help = "Stage ID (e.g. 03_review)")]
        stage_id: String,
        #[arg(short, long, help = "Feedback for rejection")]
        feedback: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let mut state = AppState::new(&cli.workspace);
    state.load_workflows().await?;

    match cli.command {
        Commands::Init {
            name,
            path,
            template,
        } => {
            state
                .init_workflow(path.as_deref(), &name, template.as_deref())
                .await?;
            println!("initialized workflow: {}", name);
        }
        Commands::Run {
            workflow_id,
            stage,
        } => {
            let workflow = state
                .get_workflow(&workflow_id)
                .ok_or_else(|| anyhow::anyhow!("workflow not found: {}", workflow_id))?;
            let plan = workflow.execution_plan();

            if plan.is_complete() {
                println!("workflow complete: {}", workflow_id);
                return Ok(());
            }

            let stage_id = match &stage {
                Some(s) => StageId::parse(s).map_err(|e| anyhow::anyhow!("{e}"))?,
                None => {
                    let next = plan
                        .next_stage()
                        .ok_or_else(|| anyhow::anyhow!("no pending stages"))?;
                    next.clone()
                }
            };

            let stage = workflow
                .stage_by_id(&stage_id)
                .ok_or_else(|| anyhow::anyhow!("stage not found: {}", stage_id.raw))?
                .clone();

            let _lock = acquire_lock(&stage.path).await?;
            clean_output(&stage.path).await?;
            println!("executing stage {} in {}", stage_id.raw, workflow_id);
            println!("  input:  {}", stage.input_path.display());
            println!("  output: {}", stage.output_path.display());

            if let Err(e) = state.execute_stage(&workflow_id, &stage).await {
                let error_marker = stage.path.join(".error");
                tokio::fs::write(&error_marker, format!("{e}"))
                    .await
                    .map_err(|io_err| anyhow::anyhow!("failed to write error marker: {io_err}"))?;
                anyhow::bail!("stage execution failed: {e}");
            }

            mark_complete(&stage.path, None).await?;
            println!("stage complete: {}", stage_id.raw);
        }
        Commands::Status { workflow_id: None } => {
            let workflows = state.list_workflows();
            if workflows.is_empty() {
                println!("no workflows");
                return Ok(());
            }
            for (id, status) in workflows {
                println!("{}\t{}", id, status);
            }
        }
        Commands::Status {
            workflow_id: Some(wid),
        } => {
            let workflow = state
                .get_workflow(&wid)
                .ok_or_else(|| anyhow::anyhow!("workflow not found: {}", wid))?;
            let plan = workflow.execution_plan();
            let complete = plan.is_complete();
            let next = plan.next_stage().map(|s| s.raw.as_str()).unwrap_or("none");
            println!("id:       {}", workflow.id);
            println!("root:     {}", workflow.root.display());
            println!("stages:   {}", workflow.stages.len());
            println!("complete: {}", complete);
            println!("next:     {}", next);
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
        Commands::List => {
            let workflows = state.list_workflows();
            if workflows.is_empty() {
                println!("no workflows");
                return Ok(());
            }
            for (id, status) in workflows {
                println!("{}\t{}", id, status);
            }
        }
        Commands::Approve {
            workflow_id,
            stage_id,
        } => {
            state
                .mark_stage_complete(&workflow_id, &stage_id)
                .await?;
            println!("approved: {} / {}", workflow_id, stage_id);
        }
        Commands::Reject {
            workflow_id,
            stage_id,
            feedback,
        } => {
            state
                .mark_stage_error(&workflow_id, &stage_id, feedback.as_deref())
                .await?;
            println!("rejected: {} / {}", workflow_id, stage_id);
        }
    }

    Ok(())
}
