use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "zerochain",
    version,
    about = "Filesystem-native workflow engine — build AI agents with mkdir",
    after_long_help = "Stage config: CONTEXT.md (YAML) or CONTEXT.lua (Lua script)\n\
                       Docs: https://github.com/awdemos/zerochain"
)]
pub struct Cli {
    #[arg(long, env = "ZEROCHAIN_WORKSPACE", default_value = "./workspace")]
    pub workspace: PathBuf,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "Create a new workflow with numbered stages")]
    Init {
        #[arg(short, long, help = "Workflow name")]
        name: String,
        #[arg(short, long, help = "Path to workspace root")]
        path: Option<PathBuf>,
        #[arg(
            short,
            long,
            help = "Comma-separated stage names (e.g. \"research,design,implement\")"
        )]
        template: Option<String>,
        #[arg(short, long, help = "Overwrite an existing workflow")]
        force: bool,
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
    #[command(about = "List available workflow templates")]
    Templates,
    #[command(about = "Start MCP server over stdio for AI tool integration")]
    Mcp,
}
