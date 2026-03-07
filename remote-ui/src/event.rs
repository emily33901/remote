use remote::{ConnectionId, PeerEvent, PeerId};
use tokio::sync::mpsc::Sender;

#[derive(Debug, Clone)]
pub enum AppEvent {
    PeerCreated {
        local_id: PeerId,
        command_tx: Sender<AppCommand>,
    },
    PeerEvent {
        local_id: PeerId,
        event: PeerEvent,
    },
    PeerCreationFailed {
        error: String,
    },
    PeerDestroyed {
        local_id: PeerId,
    },
}

#[derive(Debug, Clone)]
pub enum AppCommand {
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
