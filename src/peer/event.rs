use crate::protocol::{StreamRequest, StreamRequestResponse};
use crate::types::{ConnectionId, PeerId};
use tokio::sync::oneshot;

#[derive(Debug)]
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
        request: StreamRequest,
        response_tx: oneshot::Sender<StreamRequestResponse>,
    },
    StreamResponse {
        peer_id: PeerId,
        response: StreamRequestResponse,
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
