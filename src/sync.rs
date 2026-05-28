//! Clipboard sync logic — deduplication, echo suppression, and coordination
//! between local clipboard monitor and remote peers.

use crate::clipboard::{read_once, set_content};
use crate::message::ClipboardContent;
use crate::transport::{ClipboardEvent, PeerManager};
use std::sync::Arc;
use tokio::sync::Mutex;

fn is_empty(content: &ClipboardContent) -> bool {
    match content {
        ClipboardContent::Text(s) => s.is_empty(),
        ClipboardContent::Image { data, .. } => data.is_empty(),
    }
}

/// Tracks sync state to suppress echo loops.
struct SyncState {
    /// Hash of the content we last wrote to the local clipboard from remote.
    last_remote_hash: Option<[u8; 32]>,
}

impl SyncState {
    fn new() -> Self {
        Self {
            last_remote_hash: None,
        }
    }
}

/// The sync coordinator, receiving events from both local monitor and remote peers.
pub struct SyncCoordinator {
    manager: PeerManager,
    state: Arc<Mutex<SyncState>>,
}

impl SyncCoordinator {
    pub fn new(manager: PeerManager) -> Self {
        Self {
            manager,
            state: Arc::new(Mutex::new(SyncState::new())),
        }
    }

    /// Run the main sync loop.
    ///
    /// Bridges the sync `mpsc::Receiver` from the clipboard monitor thread into
    /// the async world, then runs an event loop receiving from both local
    /// monitor and remote peers.
    pub async fn run(self, local_rx: std::sync::mpsc::Receiver<ClipboardContent>) {
        // Bridge: blocking thread → tokio mpsc channel
        let (tx, mut local_async) = tokio::sync::mpsc::unbounded_channel();
        std::thread::spawn(move || {
            for content in local_rx {
                if tx.send(content).is_err() {
                    break; // receiver dropped
                }
            }
        });

        let mut event_rx = self.manager.subscribe();

        loop {
            tokio::select! {
                // Remote events from WS
                Ok(event) = event_rx.recv() => {
                    self.handle_remote_event(event).await;
                }

                // Local clipboard changes (bridged from monitor thread)
                Some(content) = local_async.recv() => {
                    self.handle_local_change(content).await;
                }

                else => break,
            }
        }
    }

    /// Handle a clipboard event from a remote peer.
    async fn handle_remote_event(&self, event: ClipboardEvent) {
        // Guard: ignore empty clipboard.
        if is_empty(&event.content) {
            log::debug!("Ignoring empty remote clipboard content");
            return;
        }

        let content_hash = event.content.hash();

        // Check if we just set this ourselves (echo suppression).
        {
            let state = self.state.lock().await;
            if state.last_remote_hash == Some(content_hash) {
                log::debug!("Suppressed echo: content matches last remote write");
                return;
            }
        }

        // Check if local clipboard already has this content.
        if let Some(local) = read_once() {
            if local.hash() == content_hash {
                log::debug!("Skipping remote change: local clipboard already matches");
                return;
            }
        }

        // Apply to local clipboard.
        log::info!(
            "Applying remote clipboard change ({} bytes)",
            match &event.content {
                ClipboardContent::Text(s) => s.len(),
                ClipboardContent::Image { data, .. } => data.len(),
            }
        );

        set_content(event.content.clone());

        // Record the hash to suppress the echo from our own monitor.
        {
            let mut state = self.state.lock().await;
            state.last_remote_hash = Some(content_hash);
        }

        // Clear the suppression after a short delay (allowing the monitor
        // to fire and be suppressed).
        let state = self.state.clone();
        let hash = content_hash;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            let mut state = state.lock().await;
            if state.last_remote_hash == Some(hash) {
                state.last_remote_hash = None;
            }
        });
    }

    /// Handle a local clipboard change — broadcast to peers.
    async fn handle_local_change(&self, content: ClipboardContent) {
        // Guard: ignore empty clipboard.
        if is_empty(&content) {
            return;
        }

        // Check if this was triggered by a remote write we just applied.
        let content_hash = content.hash();
        {
            let state = self.state.lock().await;
            if state.last_remote_hash == Some(content_hash) {
                log::debug!("Suppressed local broadcast: echo of remote write");
                return;
            }
        }

        log::info!(
            "Local clipboard changed, broadcasting ({} bytes)",
            match &content {
                ClipboardContent::Text(s) => s.len(),
                ClipboardContent::Image { data, .. } => data.len(),
            }
        );

        // Broadcast to all peers.
        self.manager.broadcast(&content, None).await;
    }
}
