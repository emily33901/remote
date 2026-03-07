use remote::{ConnectionId, PeerEvent, PeerId};

#[derive(Debug, Clone)]
pub enum AppEvent {
    PeerCreated { local_id: PeerId },
    PeerEvent(PeerEvent),
    PeerCreationFailed { error: String },
    PeerDestroyed,
}

#[derive(Debug, Clone)]
pub enum AppCommand {
    CreatePeer,
    ConnectToPeer {
        peer_id: PeerId,
    },
    AcceptConnection {
        peer_id: PeerId,
        connection_id: ConnectionId,
    },
    Disconnect {
        peer_id: PeerId,
    },
    Shutdown,
}
