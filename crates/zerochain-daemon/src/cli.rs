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

#[derive(Subcommand, Debug)]
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
        #[arg(
            long = "parent",
            help = "Parent contribution ID to build on (repeatable)"
        )]
        parents: Vec<String>,
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
    #[command(about = "Export a workflow as an OKF v0.2 bundle")]
    ExportOkf {
        #[arg(help = "Workflow ID")]
        workflow_id: String,
        #[arg(short, long, help = "Output directory for the OKF bundle")]
        output: Option<PathBuf>,
    },
    #[command(about = "Publish a contribution to the workspace graph")]
    Contribute {
        #[arg(
            long = "type",
            help = "Contribution type: insight, hypothesis, or report"
        )]
        kind: String,
        #[arg(short, long, help = "Markdown body of the contribution")]
        body: String,
        #[arg(long = "parent", help = "Parent contribution ID (repeatable)")]
        parents: Vec<String>,
        #[arg(long = "tag", help = "Tag (repeatable)")]
        tags: Vec<String>,
        #[arg(long, help = "Metric spec: name=bpb,value=1.9,direction=lower")]
        metric: Option<String>,
    },
    #[command(about = "Publish a verification verdict for a contribution")]
    Verify {
        #[arg(help = "Target contribution ID")]
        target: String,
        #[arg(long, help = "Verdict: confirmed, partial, or failed")]
        verdict: String,
        #[arg(short, long, help = "Evidence body")]
        body: String,
    },
    #[command(about = "Inspect the workspace contribution graph")]
    Graph {
        #[arg(
            long,
            help = "View: recent, leaves, open_hypotheses, unverified, negative, leaders"
        )]
        view: Option<String>,
        #[arg(long = "type", help = "Filter by contribution type")]
        record_type: Option<String>,
        #[arg(long, help = "Filter by workflow")]
        workflow: Option<String>,
        #[arg(long, help = "Emit JSON")]
        json: bool,
    },
    #[command(about = "Start MCP server over stdio for AI tool integration")]
    Mcp,
}
