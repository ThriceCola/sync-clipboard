//! WebSocket peer-to-peer transport layer.
//!
//! Each node runs a WS server (for incoming connections) and optionally
//! connects out to other peers. All connections are treated equally;
//! clipboard changes are broadcast to all connected peers.

use crate::message::{ClipboardContent, deserialize, serialize};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, broadcast};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type WsWrite = futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
type WsRead = futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

/// Unique identifier for a connected peer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerId(pub SocketAddr);

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Events coming from the transport layer.
#[derive(Debug, Clone)]
pub struct ClipboardEvent {
    pub content: ClipboardContent,
    /// The peer that sent this (None if from local clipboard).
    #[allow(dead_code)]
    pub from_peer: Option<PeerId>,
}

/// Manages WebSocket connections to all peers.
///
/// Cloneable handle; internal state is behind `Arc<Mutex<...>>`.
#[derive(Clone)]
pub struct PeerManager {
    inner: Arc<PeerManagerInner>,
}

struct PeerManagerInner {
    /// Map of connected peers to their WS write halves.
    peers: Mutex<HashMap<PeerId, WsWrite>>,
    /// Channel for sending clipboard events to the sync layer.
    event_tx: broadcast::Sender<ClipboardEvent>,
}

impl PeerManager {
    /// Create a new PeerManager.
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(PeerManagerInner {
                peers: Mutex::new(HashMap::new()),
                event_tx,
            }),
        }
    }

    /// Get a receiver for clipboard events (local and remote).
    pub fn subscribe(&self) -> broadcast::Receiver<ClipboardEvent> {
        self.inner.event_tx.subscribe()
    }

    /// Start the WebSocket server on the given address.
    pub async fn serve(self, addr: SocketAddr) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        log::info!("Listening on ws://{addr}");

        loop {
            let (stream, peer_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    log::error!("Accept error: {e}");
                    continue;
                }
            };

            let stream = MaybeTlsStream::Plain(stream);
            let ws_stream = tokio_tungstenite::accept_async(stream)
                .await
                .context("WebSocket handshake")?;

            log::info!("Peer connected: {peer_addr}");
            let peer_id = PeerId(peer_addr);
            let (write, read) = ws_stream.split();
            self.inner.peers.lock().await.insert(peer_id.clone(), write);

            // Spawn per-connection read loop for accepted connections.
            let mgr = self.clone();
            tokio::spawn(async move {
                mgr.read_loop(peer_id, read).await;
            });
        }
    }

    /// Connect to a remote peer and run the read loop until disconnection.
    pub async fn connect_to(self, addr: SocketAddr) -> Result<()> {
        log::info!("Connecting to ws://{addr}");

        let (ws_stream, _) = connect_async(format!("ws://{addr}"))
            .await
            .context("WebSocket connect")?;

        let peer_id = PeerId(addr);
        log::info!("Connected to peer: {addr}");
        let (write, read) = ws_stream.split();
        self.inner.peers.lock().await.insert(peer_id.clone(), write);

        // Run read loop inline — blocks until connection drops.
        self.read_loop(peer_id, read).await;

        Ok(())
    }

    /// Connect to a peer with automatic reconnection.
    pub fn connect_with_retry(self, addr: SocketAddr) {
        tokio::spawn(async move {
            loop {
                if let Err(e) = self.clone().connect_to(addr).await {
                    log::warn!("Connection to {addr} failed: {e}, retrying in 3s...");
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                } else {
                    log::info!("Connection to {addr} lost, reconnecting in 1s...");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        });
    }

    /// Broadcast clipboard content to all connected peers.
    pub async fn broadcast(&self, content: &ClipboardContent, exclude: Option<&PeerId>) {
        let data = serialize(content);
        let msg = Message::Binary(data.into());
        let mut peers = self.inner.peers.lock().await;

        let mut to_remove = Vec::new();

        for (peer_id, write) in peers.iter_mut() {
            if exclude == Some(peer_id) {
                continue;
            }
            if let Err(e) = write.send(msg.clone()).await {
                log::warn!("Failed to send to {peer_id}: {e}");
                to_remove.push(peer_id.clone());
            }
        }

        for id in to_remove {
            peers.remove(&id);
            log::info!("Removed disconnected peer: {id}");
        }
    }

    /// Notify sync layer about a remote clipboard change.
    fn notify_remote(&self, content: ClipboardContent, peer: PeerId) {
        let event = ClipboardEvent {
            content,
            from_peer: Some(peer),
        };
        let _ = self.inner.event_tx.send(event);
    }

    /// Read loop for a peer connection. Runs until the connection drops.
    async fn read_loop(self, peer_id: PeerId, mut read: WsRead) {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Binary(data)) => {
                    if let Some(content) = deserialize(&data) {
                        log::debug!(
                            "Received from {peer_id}: {}",
                            if content.is_image() {
                                let len = match &content {
                                    ClipboardContent::Image { data, .. } => data.len(),
                                    _ => 0,
                                };
                                format!("Image ({len} bytes)")
                            } else {
                                let len = match &content {
                                    ClipboardContent::Text(s) => s.len(),
                                    _ => 0,
                                };
                                format!("Text ({len} chars)")
                            }
                        );
                        self.notify_remote(content, peer_id.clone());
                    } else {
                        log::warn!("Failed to deserialize message from {peer_id}");
                    }
                }
                Ok(Message::Close(_)) => {
                    log::info!("Peer {peer_id} closed connection");
                    break;
                }
                Ok(Message::Ping(_data)) => {
                    // tokio-tungstenite handles ping/pong automatically
                }
                Ok(_) => {} // ignore text, pong etc
                Err(e) => {
                    log::warn!("Read error from {peer_id}: {e}");
                    break;
                }
            }
        }

        // Connection closed, remove peer from map.
        self.inner.peers.lock().await.remove(&peer_id);
        log::info!("Peer {peer_id} disconnected");
    }
}

impl Default for PeerManager {
    fn default() -> Self {
        Self::new()
    }
}
