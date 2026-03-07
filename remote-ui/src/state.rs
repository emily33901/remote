use std::collections::HashMap;

use remote::{ConnectionId, PeerId, StreamRequest, StreamRequestResponse};
use tokio::sync::mpsc::Sender;

use crate::event::AppCommand;

#[derive(Debug, Clone)]
pub struct PendingConnection {
    pub peer_id: PeerId,
    pub connection_id: ConnectionId,
}

#[derive(Debug, Clone)]
pub struct PendingStreamRequest {
    pub request_id: u64,
    pub peer_id: PeerId,
    pub request: StreamRequest,
}

#[derive(Debug, Clone)]
pub struct OutgoingStreamRequest {
    pub peer_id: PeerId,
    pub status: OutgoingStreamRequestStatus,
}

#[derive(Debug, Clone)]
pub enum OutgoingStreamRequestStatus {
    Pending,
    Responded(StreamRequestResponse),
}

#[derive(Debug, Clone, Default)]
pub struct LocalPeerState {
    pub connected_peers: Vec<PeerId>,
    pub pending_connections: Vec<PendingConnection>,
    pub pending_stream_requests: Vec<PendingStreamRequest>,
    pub outgoing_stream_requests: Vec<OutgoingStreamRequest>,
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

    pub fn apply_peer_connected(&mut self, local_id: &PeerId, peer_id: &PeerId) {
        if let Some(state) = self.local_peers.get_mut(local_id) {
            state.pending_connections.retain(|p| &p.peer_id != peer_id);
            if !state.connected_peers.contains(peer_id) {
                state.connected_peers.push(peer_id.clone());
            }
        }
    }

    pub fn apply_peer_disconnected(&mut self, local_id: &PeerId, peer_id: &PeerId) {
        if let Some(state) = self.local_peers.get_mut(local_id) {
            state.connected_peers.retain(|id| id != peer_id);
            state.pending_connections.retain(|p| &p.peer_id != peer_id);
            state
                .pending_stream_requests
                .retain(|r| &r.peer_id != peer_id);
            state
                .outgoing_stream_requests
                .retain(|r| &r.peer_id != peer_id);
        }
    }

    pub fn apply_connection_requested(
        &mut self,
        local_id: &PeerId,
        peer_id: &PeerId,
        connection_id: &ConnectionId,
    ) {
        if let Some(state) = self.local_peers.get_mut(local_id) {
            state.pending_connections.push(PendingConnection {
                peer_id: peer_id.clone(),
                connection_id: connection_id.clone(),
            });
        }
    }

    pub fn apply_stream_request_received(
        &mut self,
        local_id: &PeerId,
        peer_id: &PeerId,
        request_id: u64,
        request: StreamRequest,
    ) {
        tracing::info!(
            "apply_stream_request_received: local_id={}, peer_id={}, request_id={}",
            local_id,
            peer_id,
            request_id
        );
        if let Some(state) = self.local_peers.get_mut(local_id) {
            tracing::info!("Found state for local_id, adding stream request");
            state.pending_stream_requests.push(PendingStreamRequest {
                request_id,
                peer_id: peer_id.clone(),
                request,
            });
        } else {
            tracing::warn!("No state found for local_id={}", local_id);
        }
    }

    pub fn apply_stream_response_received(
        &mut self,
        local_id: &PeerId,
        peer_id: &PeerId,
        response: &StreamRequestResponse,
    ) {
        if let Some(state) = self.local_peers.get_mut(local_id) {
            if let Some(req) = state
                .outgoing_stream_requests
                .iter_mut()
                .find(|r| r.peer_id == *peer_id)
            {
                req.status = OutgoingStreamRequestStatus::Responded(response.clone());
            }
        }
    }

    pub fn remove_stream_request(&mut self, local_id: &PeerId, request_id: u64) {
        if let Some(state) = self.local_peers.get_mut(local_id) {
            state
                .pending_stream_requests
                .retain(|r| r.request_id != request_id);
        }
    }

    pub fn add_outgoing_stream_request(&mut self, local_id: &PeerId, peer_id: PeerId) {
        if let Some(state) = self.local_peers.get_mut(local_id) {
            if !state
                .outgoing_stream_requests
                .iter()
                .any(|r| r.peer_id == peer_id)
            {
                state.outgoing_stream_requests.push(OutgoingStreamRequest {
                    peer_id,
                    status: OutgoingStreamRequestStatus::Pending,
                });
            }
        }
    }
}

pub struct PeerHandle {
    pub command_tx: Sender<AppCommand>,
}
