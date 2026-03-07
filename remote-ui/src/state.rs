use std::collections::HashMap;

use remote::{ConnectionId, PeerId};
use tokio::sync::mpsc::Sender;

use crate::event::AppCommand;

#[derive(Debug, Clone)]
pub struct PendingConnection {
    pub peer_id: PeerId,
    pub connection_id: ConnectionId,
}

#[derive(Debug, Clone, Default)]
pub struct LocalPeerState {
    pub connected_peers: Vec<PeerId>,
    pub pending_connections: Vec<PendingConnection>,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteUIState {
    pub local_peers: HashMap<PeerId, LocalPeerState>,
    pub last_error: Option<String>,
}

impl RemoteUIState {
    pub fn add_local_peer(&mut self, local_id: PeerId) {
        self.local_peers.insert(local_id, LocalPeerState::default());
    }

    pub fn remove_local_peer(&mut self, local_id: &PeerId) {
        self.local_peers.remove(local_id);
    }

    pub fn apply_peer_event(&mut self, local_id: &PeerId, event: remote::PeerEvent) {
        let Some(state) = self.local_peers.get_mut(local_id) else {
            return;
        };

        match event {
            remote::PeerEvent::ConnectionRequested {
                peer_id,
                connection_id,
            } => {
                state.pending_connections.push(PendingConnection {
                    peer_id,
                    connection_id,
                });
            }
            remote::PeerEvent::PeerConnected { peer_id } => {
                state.pending_connections.retain(|p| p.peer_id != peer_id);
                if !state.connected_peers.contains(&peer_id) {
                    state.connected_peers.push(peer_id);
                }
            }
            remote::PeerEvent::PeerDisconnected { peer_id, reason: _ } => {
                state.connected_peers.retain(|id| *id != peer_id);
                state.pending_connections.retain(|p| p.peer_id != peer_id);
            }
            remote::PeerEvent::SignalMessage { .. } => {}
            remote::PeerEvent::IncomingStream { peer_id } => {
                tracing::info!(
                    "Incoming stream from {} on local peer {}",
                    peer_id,
                    local_id
                );
            }
            remote::PeerEvent::Error { error, .. } => {
                self.last_error = Some(error.clone());
            }
        }
    }
}

pub struct PeerHandle {
    pub command_tx: Sender<AppCommand>,
}
