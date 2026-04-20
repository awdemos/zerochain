use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use zerochain_broker::memory::MemoryBroker;
use zerochain_cas::CasStore;
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

    #[arg(long, env = "ZEROCHAIN_CAS_DIR", default_value = "/workspace/.zerochain/cas")]
    cas_dir: PathBuf,

    #[arg(long, env = "ZEROCHAIN_CAS_BACKEND", default_value = "local")]
    cas_backend: String,

    #[arg(long, env = "ZEROCHAIN_BROKER_ENABLED")]
    broker_enabled: bool,

    #[arg(long, env = "ZEROCHAIN_API_KEY")]
    api_key: Option<String>,
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
        cas_dir = %cli.cas_dir.display(),
        cas_backend = %cli.cas_backend,
        broker_enabled = cli.broker_enabled,
        auth_enabled = cli.api_key.is_some(),
        "starting zerochaind"
    );

    let mut server_state = state::ServerState::new(&cli.workspace);
    if let Some(key) = cli.api_key {
        server_state = server_state.with_api_key(key);
    }

    let cas: CasStore = if cli.cas_backend.as_str() == "s3" {
        #[cfg(feature = "s3")]
        {
            let backend = zerochain_cas::S3Backend::from_env()?;
            let store = CasStore::with_backend(backend);
            tracing::info!("CAS backend: s3 ({})", store.location());
            store
        }
        #[cfg(not(feature = "s3"))]
        {
            anyhow::bail!("S3 CAS backend requested but zerochain-cas was compiled without the 's3' feature");
        }
    } else {
        let store = CasStore::new(cli.cas_dir).await?;
        tracing::info!("CAS backend: local ({})", store.location());
        store
    };
    server_state = server_state.with_cas(cas);

    if cli.broker_enabled {
        let broker = MemoryBroker::new();
        server_state = server_state.with_broker(broker);
        tracing::info!("broker enabled (memory backend)");
    }

    server_state.refresh().await?;

    let app = routes::routes(server_state);

    let listener = tokio::net::TcpListener::bind(&cli.listen).await?;
    tracing::info!("listening on {}", cli.listen);

    axum::serve(listener, app).await?;

    Ok(())
}
