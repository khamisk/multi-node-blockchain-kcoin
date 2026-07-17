use std::collections::HashSet;

use anyhow::{Context, Result};
use clap::Parser;
use kcoin_node::{
    api,
    config::{NodeArgs, NodeConfig},
    network::spawn_network,
    runtime::start_node,
    storage::Store,
};
use libp2p::Multiaddr;
use tokio::net::TcpListener;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let config = NodeConfig::try_from(NodeArgs::parse())?;
    let store = Store::open(&config.db_path)
        .with_context(|| format!("open node database {}", config.db_path.display()))?;
    let bootstrap = config
        .peers
        .iter()
        .map(|peer| {
            peer.parse::<Multiaddr>()
                .with_context(|| format!("invalid peer {peer}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let network = match spawn_network(
        config.chain_id.clone(),
        config.p2p_port,
        bootstrap,
        HashSet::new(),
    )
    .await
    {
        Ok(network) => Some(network),
        Err(error) if config.demo => {
            warn!(%error, "network unavailable; demo node will continue standalone");
            None
        }
        Err(error) => return Err(error).context("start libp2p network"),
    };

    let handle = start_node(config.clone(), store, network).await?;
    let app = api::router(handle.clone());
    let listener = TcpListener::bind(config.api_addr)
        .await
        .with_context(|| format!("bind API to {}", config.api_addr))?;
    info!(address = %config.api_addr, "KCoin API ready");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            handle.shutdown().await;
        })
        .await
        .context("serve HTTP API")
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("kcoin_node=info,tower_http=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    info!("shutdown requested");
}
