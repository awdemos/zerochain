use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::JoinHandle;

use zerochain_broker::Broker;
use zerochain_cas::CasStore;
use zerochain_server::routes;
use zerochain_server::state;
use zerochain_server::subscriber;

/// Wait for a shutdown signal (Ctrl+C or SIGTERM on Unix).
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received Ctrl+C, shutting down gracefully"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down gracefully"),
    }
}

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

    #[arg(
        long,
        env = "ZEROCHAIN_CAS_DIR",
        default_value = "/workspace/.zerochain/cas"
    )]
    cas_dir: PathBuf,

    #[arg(long, env = "ZEROCHAIN_CAS_BACKEND", default_value = "local")]
    cas_backend: String,

    #[arg(long, env = "ZEROCHAIN_BROKER_ENABLED")]
    broker_enabled: bool,

    #[arg(long, env = "ZEROCHAIN_BROKER_BACKEND", default_value = "memory")]
    broker_backend: String,

    #[arg(long, env = "ZEROCHAIN_API_KEY")]
    api_key: Option<String>,

    #[arg(
        long,
        env = "ZEROCHAIN_NO_AUTH",
        help = "Explicitly disable API key authentication"
    )]
    no_auth: bool,

    #[arg(
        long,
        env = "ZEROCHAIN_MAX_BODY_SIZE",
        default_value = "1048576",
        help = "Maximum HTTP request body size in bytes (default: 1 MiB)"
    )]
    max_body_size: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zerochaind=info".into()),
        )
        .try_init()
        .ok();

    let cli = Cli::parse();

    tracing::info!(
        listen = %cli.listen,
        workspace = %cli.workspace.display(),
        cas_dir = %cli.cas_dir.display(),
        cas_backend = %cli.cas_backend,
        broker_enabled = cli.broker_enabled,
        broker_backend = %cli.broker_backend,
        auth_enabled = cli.api_key.is_some() && !cli.no_auth,
        "starting zerochaind"
    );

    let mut server_state = state::ServerState::new(&cli.workspace).await;
    if cli.no_auth {
        server_state = server_state.with_auth_disabled();
    }
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
            anyhow::bail!(
                "S3 CAS backend requested but zerochain-cas was compiled without the 's3' feature"
            );
        }
    } else {
        let store = CasStore::new(cli.cas_dir).await?;
        tracing::info!("CAS backend: local ({})", store.location());
        store
    };
    server_state = server_state.with_cas(cas.clone());
    {
        let guard = server_state.registry.read().await;
        guard.set_cas(cas.clone()).await;
    }

    let mut subscriber_handle: Option<JoinHandle<()>> = None;
    if cli.broker_enabled {
        let broker: Arc<dyn Broker> = match cli.broker_backend.as_str() {
            "nats" => {
                #[cfg(feature = "nats")]
                {
                    let nats = zerochain_broker::nats::NatsBroker::from_env().await?;
                    tracing::info!("broker backend: nats");
                    Arc::new(nats)
                }
                #[cfg(not(feature = "nats"))]
                {
                    anyhow::bail!("NATS broker backend requested but zerochain-broker was compiled without the 'nats' feature");
                }
            }
            _ => {
                let memory = zerochain_broker::memory::MemoryBroker::new();
                tracing::info!("broker backend: memory");
                Arc::new(memory)
            }
        };
        server_state = server_state.with_broker(broker.clone());

        // Spawn background subscriber that bridges broker messages into stage input directories.
        subscriber_handle = Some(tokio::spawn(subscriber::spawn(
            cas,
            broker,
            cli.workspace.clone(),
        )));
    }

    server_state.refresh().await?;

    let app =
        routes::routes(server_state).layer(axum::extract::DefaultBodyLimit::max(cli.max_body_size));

    let listener = tokio::net::TcpListener::bind(&cli.listen).await?;
    tracing::info!("listening on {}", cli.listen);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    if let Some(handle) = subscriber_handle {
        handle.abort();
        let _ = handle.await;
    }

    tracing::info!("zerochaind stopped");
    Ok(())
}
