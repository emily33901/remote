use remote::{ConnectionId, PeerId, StreamRequest, StreamRequestResponse};
use tokio::sync::mpsc::Sender;

#[derive(Debug)]
pub enum AppEvent {
    PeerCreated {
        local_id: PeerId,
        command_tx: Sender<AppCommand>,
    },
    PeerCreationFailed {
        error: String,
    },
    PeerDestroyed {
        local_id: PeerId,
    },
    PeerConnected {
        local_id: PeerId,
        peer_id: PeerId,
    },
    PeerDisconnected {
        local_id: PeerId,
        peer_id: PeerId,
    },
    ConnectionRequested {
        local_id: PeerId,
        peer_id: PeerId,
        connection_id: ConnectionId,
    },
    StreamRequestReceived {
        local_id: PeerId,
        peer_id: PeerId,
        request: StreamRequest,
        request_id: u64,
    },
    StreamResponseReceived {
        local_id: PeerId,
        peer_id: PeerId,
        response: StreamRequestResponse,
    },
    PeerError {
        local_id: PeerId,
        error: String,
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
    RequestStream {
        peer_id: PeerId,
        request: StreamRequest,
    },
    RespondToStreamRequest {
        request_id: u64,
        response: StreamRequestResponse,
    },
    Shutdown,
}
