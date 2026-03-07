mod config;
mod event;

pub use config::*;
pub use event::*;

use std::collections::HashMap;

use crate::connection::{create_peer_connection, PeerConnectionControl};
use crate::error::{Error, Result};
use crate::protocol::StreamRequest;
use crate::types::{ConnectionId, PeerId};
use signal::{SignallingControl, SignallingEvent};
use tokio::sync::mpsc;
use tracing::Instrument;

const CHANNEL_LIMIT: usize = 10;

#[derive(Debug, Clone)]
pub enum SignalMessage {
    Offer(String),
    Answer(String),
    IceCandidate(String),
}

pub struct Peer {
    id: PeerId,
    config: PeerConfig,
    signal_control: mpsc::Sender<SignallingControl>,
    remote_peers: HashMap<PeerId, RemotePeerHandle>,
    pending_connections: HashMap<ConnectionId, PeerId>,
    event_tx: mpsc::Sender<PeerEvent>,
    tasks: tokio::task::JoinSet<anyhow::Result<()>>,
}

pub struct RemotePeerHandle {
    pub peer_id: PeerId,
    pub control: mpsc::Sender<PeerConnectionControl>,
}

impl Peer {
    pub async fn new(config: PeerConfig) -> Result<(Self, mpsc::Receiver<PeerEvent>)> {
        let (signal_control, mut signal_event) =
            signal::client(&config.signal_server).await.map_err(|e| {
                Error::ConnectionFailed(format!("Failed to connect to signaling: {e}"))
            })?;

        let id = async {
            loop {
                match signal_event.recv().await {
                    Some(SignallingEvent::Id(id)) => return Ok(id),
                    Some(event) => {
                        tracing::warn!(?event, "unexpected event while waiting for id");
                    }
                    None => return Err(Error::ConnectionFailed("Signal channel closed".into())),
                }
            }
        }
        .await?;

        let (event_tx, event_rx) = mpsc::channel(CHANNEL_LIMIT);
        let signal_control_clone = signal_control.clone();
        let event_tx_clone = event_tx.clone();
        let config_clone = config.clone();

        let mut peer = Self {
            id: id.clone(),
            config,
            signal_control: signal_control.clone(),
            remote_peers: HashMap::new(),
            pending_connections: HashMap::new(),
            event_tx,
            tasks: tokio::task::JoinSet::new(),
        };

        peer.tasks.spawn(
            async move {
                Self::handle_signal_events(
                    id,
                    config_clone,
                    signal_control_clone,
                    signal_event,
                    event_tx_clone,
                )
                .await
            }
            .in_current_span(),
        );

        Ok((peer, event_rx))
    }

    pub fn id(&self) -> &PeerId {
        &self.id
    }

    pub async fn connect(&mut self, peer_id: PeerId) -> Result<()> {
        if self.remote_peers.contains_key(&peer_id) {
            return Err(Error::AlreadyConnected);
        }

        self.signal_control
            .send(SignallingControl::RequestConnection(peer_id.clone()))
            .await
            .map_err(|_| Error::ConnectionFailed("Signal channel closed".into()))?;

        Ok(())
    }

    pub async fn accept_connection(&mut self, connection_id: ConnectionId) -> Result<()> {
        let Some(peer_id) = self.pending_connections.remove(&connection_id) else {
            return Err(Error::ConnectionFailed("No pending connection".into()));
        };

        self.signal_control
            .send(SignallingControl::AcceptConnection(connection_id))
            .await
            .map_err(|_| Error::ConnectionFailed("Signal channel closed".into()))?;

        let (control, _event) = create_peer_connection(
            self.config.webrtc_api,
            self.id.clone(),
            peer_id.clone(),
            self.signal_control.clone(),
            false,
        )
        .await
        .map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        let handle = RemotePeerHandle {
            peer_id: peer_id.clone(),
            control,
        };
        self.remote_peers.insert(peer_id.clone(), handle);

        self.event_tx
            .send(PeerEvent::PeerConnected { peer_id })
            .await
            .map_err(|_| Error::ConnectionFailed("Event channel closed".into()))?;

        Ok(())
    }

    pub async fn disconnect(&mut self, peer_id: &PeerId) -> Result<()> {
        if let Some(handle) = self.remote_peers.remove(peer_id) {
            let _ = handle.control.send(PeerConnectionControl::Disconnect).await;
            self.event_tx
                .send(PeerEvent::PeerDisconnected {
                    peer_id: peer_id.clone(),
                    reason: DisconnectReason::UserInitiated,
                })
                .await
                .map_err(|_| Error::ConnectionFailed("Event channel closed".into()))?;
        }
        Ok(())
    }

    pub async fn request_stream(&self, peer_id: &PeerId, request: StreamRequest) -> Result<()> {
        let Some(handle) = self.remote_peers.get(peer_id) else {
            return Err(Error::PeerNotFound(peer_id.clone()));
        };
        handle
            .control
            .send(PeerConnectionControl::RequestStream(request))
            .await
            .map_err(|_| Error::ConnectionFailed("Control channel closed".into()))?;
        Ok(())
    }

    pub fn connected_peers(&self) -> impl Iterator<Item = &PeerId> {
        self.remote_peers.keys()
    }

    async fn handle_signal_events(
        our_id: PeerId,
        config: PeerConfig,
        signal_control: mpsc::Sender<SignallingControl>,
        mut signal_event: mpsc::Receiver<SignallingEvent>,
        event_tx: mpsc::Sender<PeerEvent>,
    ) -> anyhow::Result<()> {
        while let Some(event) = signal_event.recv().await {
            match event {
                SignallingEvent::Id(_) => {
                    tracing::warn!("received duplicate peer id");
                }
                SignallingEvent::ConectionRequest(peer_id, connection_id) => {
                    let _ = event_tx
                        .send(PeerEvent::ConnectionRequested { peer_id, connection_id })
                        .await;
                }
                SignallingEvent::ConnectionAccepted(peer_id, _connection_id) => {
                    let Ok((_control, _)) = create_peer_connection(
                        config.webrtc_api,
                        our_id.clone(),
                        peer_id.clone(),
                        signal_control.clone(),
                        true,
                    )
                    .await
                    else {
                        continue;
                    };

                    let _ = event_tx.send(PeerEvent::PeerConnected { peer_id }).await;
                }
                SignallingEvent::Offer(peer_id, offer) => {
                    let _ = event_tx
                        .send(PeerEvent::SignalMessage {
                            peer_id,
                            message: SignalMessage::Offer(offer),
                        })
                        .await;
                }
                SignallingEvent::Answer(peer_id, answer) => {
                    let _ = event_tx
                        .send(PeerEvent::SignalMessage {
                            peer_id,
                            message: SignalMessage::Answer(answer),
                        })
                        .await;
                }
                SignallingEvent::IceCandidate(peer_id, candidate) => {
                    let _ = event_tx
                        .send(PeerEvent::SignalMessage {
                            peer_id,
                            message: SignalMessage::IceCandidate(candidate),
                        })
                        .await;
                }
                SignallingEvent::Error(err) => {
                    tracing::error!("Signaling error: {err:?}");
                }
            }
        }

        Ok(())
    }
}
