mod event;
mod state;

pub use event::*;
pub use state::*;

use crate::types::PeerId;
use tokio::sync::mpsc;

pub struct RemotePeer {
    peer_id: PeerId,
    state: RemotePeerState,
    event_rx: mpsc::Receiver<RemotePeerEvent>,
    event_tx: mpsc::Sender<RemotePeerEvent>,
}

impl RemotePeer {
    pub fn new(peer_id: PeerId) -> Self {
        let (event_tx, event_rx) = mpsc::channel(10);
        Self {
            peer_id,
            state: RemotePeerState::Connecting,
            event_tx,
            event_rx,
        }
    }

    pub fn id(&self) -> &PeerId {
        &self.peer_id
    }

    pub fn state(&self) -> &RemotePeerState {
        &self.state
    }

    pub fn events(&mut self) -> &mut mpsc::Receiver<RemotePeerEvent> {
        &mut self.event_rx
    }

    pub async fn next_event(&mut self) -> Option<RemotePeerEvent> {
        self.event_rx.recv().await
    }

    pub async fn disconnect(self) -> crate::error::Result<()> {
        Ok(())
    }
}

impl std::fmt::Debug for RemotePeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemotePeer")
            .field("peer_id", &self.peer_id)
            .field("state", &self.state)
            .finish()
    }
}

pub struct RemotePeerHandle {
    pub peer_id: PeerId,
    pub control: mpsc::Sender<super::connection::PeerConnectionControl>,
}
