//! Synchronize clipboard between machines over WebSocket.

/// Detect image format from magic bytes.
fn detect_image_format(data: &[u8]) -> &'static str {
    if data.len() < 4 {
        return "too short";
    }
    if &data[..2] == b"BM" {
        return "BMP";
    }
    if &data[..4] == b"\x89PNG" {
        return "PNG";
    }
    if &data[..2] == b"\xff\xd8" {
        return "JPEG";
    }
    if &data[..4] == b"RIFF" && data.len() >= 12 && &data[8..12] == b"WEBP" {
        return "WebP";
    }
    if &data[..4] == b"GIF8" {
        return "GIF";
    }
    if &data[..4] == b"\x00\x00\x01\x00" {
        return "ICO";
    }
    // DIB header starts with BITMAPINFOHEADER size (40 = 0x28)
    if data.len() >= 4 && u32::from_le_bytes([data[0], data[1], data[2], data[3]]) == 40 {
        return "DIB (BITMAPINFOHEADER)";
    }
    "unknown"
}
/// Usage:
///   sync-clipboard --listen 0.0.0.0:9000
///   sync-clipboard --listen 0.0.0.0:9000 --connect 192.168.1.100:9000
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

    /// Debug: read and print current clipboard content, then exit.
    #[arg(long)]
    debug_clipboard: bool,
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

    // Debug mode: read clipboard once, print details, and exit.
    if cli.debug_clipboard {
        match clipboard::read_once() {
            Some(content) => {
                println!("Clipboard content:");
                match &content {
                    message::ClipboardContent::Text(s) => {
                        println!("  Type: Text");
                        println!("  Length: {} chars", s.chars().count());
                        let preview: String = s.chars().take(200).collect();
                        println!("  Preview: {preview}");
                    }
                    message::ClipboardContent::Image { mime_type, data } => {
                        println!("  Type: Image");
                        println!("  MIME: {mime_type}");
                        println!("  Data length: {} bytes", data.len());
                        let preview_len = data.len().min(128);
                        println!(
                            "  First {} bytes: {:02x?}",
                            preview_len,
                            &data[..preview_len]
                        );
                        // Try to detect the actual format from magic bytes.
                        let format_hint = detect_image_format(data);
                        println!("  Detected format: {format_hint}");
                    }
                }
                let hash = content.hash();
                let hash_hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
                println!("  SHA-256: {hash_hex}");
            }
            None => {
                println!("Clipboard is empty or could not be read.");
            }
        }
        return Ok(());
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
