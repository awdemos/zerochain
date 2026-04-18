use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use zerochain_server::routes;
use zerochain_server::state;

#[derive(Parser)]
#[command(
    name = "zerochaind",
    version,
    about = "HTTP daemon for containerized zerochain workflow execution"
)]
struct Cli {
    #[arg(long, env = "ZEROCHAIN_LISTEN", default_value = "0.0.0.0:8080")]
    listen: String,

    #[arg(long, env = "ZEROCHAIN_WORKSPACE", default_value = "/workspace")]
    workspace: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zerochaind=info".into()),
        )
        .init();

    let cli = Cli::parse();

    tracing::info!(
        listen = %cli.listen,
        workspace = %cli.workspace.display(),
        "starting zerochaind"
    );

    let server_state = state::ServerState::new(&cli.workspace);
    server_state.refresh().await?;

    let app = routes::routes(server_state);

    let listener = tokio::net::TcpListener::bind(&cli.listen).await?;
    tracing::info!("listening on {}", cli.listen);

    axum::serve(listener, app).await?;

    Ok(())
}
