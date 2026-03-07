use remote::{ConnectionId, PeerId};

#[derive(Debug, Clone, Default)]
pub enum UIState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct PendingConnection {
    pub peer_id: PeerId,
    pub connection_id: ConnectionId,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteUIState {
    pub state: UIState,
    pub local_peer_id: Option<PeerId>,
    pub connected_peers: Vec<PeerId>,
    pub pending_connections: Vec<PendingConnection>,
}

impl RemoteUIState {
    pub fn apply_peer_event(&mut self, event: remote::PeerEvent) {
        match event {
            remote::PeerEvent::ConnectionRequested {
                peer_id,
                connection_id,
            } => {
                self.pending_connections.push(PendingConnection {
                    peer_id,
                    connection_id,
                });
            }
            remote::PeerEvent::PeerConnected { peer_id } => {
                self.pending_connections.retain(|p| p.peer_id != peer_id);
                if !self.connected_peers.contains(&peer_id) {
                    self.connected_peers.push(peer_id);
                }
                self.state = UIState::Connected;
            }
            remote::PeerEvent::PeerDisconnected { peer_id, reason: _ } => {
                self.connected_peers.retain(|id| *id != peer_id);
                self.pending_connections.retain(|p| p.peer_id != peer_id);
                if self.connected_peers.is_empty() {
                    self.state = UIState::Disconnected;
                }
            }
            remote::PeerEvent::SignalMessage { .. } => {}
            remote::PeerEvent::IncomingStream { peer_id } => {
                tracing::info!("Incoming stream from {}", peer_id);
            }
            remote::PeerEvent::Error { error, .. } => {
                self.state = UIState::Error(error.clone());
            }
        }
    }
}
