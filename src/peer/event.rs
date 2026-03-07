use crate::types::{ConnectionId, PeerId};

#[derive(Debug, Clone)]
pub enum PeerEvent {
    ConnectionRequested {
        peer_id: PeerId,
        connection_id: ConnectionId,
    },
    PeerConnected {
        peer_id: PeerId,
    },
    PeerDisconnected {
        peer_id: PeerId,
        reason: DisconnectReason,
    },
    SignalMessage {
        peer_id: PeerId,
        message: super::SignalMessage,
    },
    IncomingStream {
        peer_id: PeerId,
    },
    Error {
        peer_id: Option<PeerId>,
        error: String,
    },
}

#[derive(Debug, Clone)]
pub enum DisconnectReason {
    UserInitiated,
    ConnectionFailed,
    RemoteClosed,
    Timeout,
}
