//! Synchronize clipboard between machines over WebSocket.
//!
//! Usage:
//!   sync-clipboard --listen 0.0.0.0:9000
//!   sync-clipboard --listen 0.0.0.0:9000 --connect 192.168.1.100:9000

mod clipboard;
mod message;
mod sync;
mod transport;

use clap::Parser;
use std::net::SocketAddr;

#[derive(Parser, Debug)]
#[command(
    name = "sync-clipboard",
    about = "Synchronize clipboard between machines"
)]
struct Cli {
    /// Address to listen for incoming WebSocket connections.
    #[arg(short, long, default_value = "0.0.0.0:9000")]
    listen: SocketAddr,

    /// Remote peers to connect to (can be specified multiple times).
    #[arg(short, long)]
    connect: Vec<SocketAddr>,

    /// Enable verbose logging.
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize logging.
    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level))
        .format_timestamp_millis()
        .init();

    log::info!("sync-clipboard starting");
    log::info!("Listening on: {}", cli.listen);
    if !cli.connect.is_empty() {
        log::info!("Peers: {:?}", cli.connect);
    }

    // Start clipboard monitor (platform-specific).
    let (local_tx, local_rx) = std::sync::mpsc::channel();
    clipboard::start_monitor(local_tx);

    // Set up transport.
    let manager = transport::PeerManager::new();
    let sync_coordinator = sync::SyncCoordinator::new(manager.clone());

    // Spawn WS server.
    let serve_manager = manager.clone();
    let serve_addr = cli.listen;
    tokio::spawn(async move {
        if let Err(e) = serve_manager.serve(serve_addr).await {
            log::error!("Server error: {e}");
        }
    });

    // Connect to peers with retry.
    for addr in cli.connect {
        manager.clone().connect_with_retry(addr);
    }

    // Run sync loop.
    sync_coordinator.run(local_rx).await;

    Ok(())
}
