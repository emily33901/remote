mod config;
mod event;

pub use config::*;
pub use event::*;

use std::collections::HashMap;
use std::sync::Arc;

use crate::connection::{create_peer_connection, PeerConnectionControl, PeerConnectionEvent};
use crate::error::{Error, Result};
use crate::protocol::StreamRequest;
use crate::types::{ConnectionId, PeerId};
use signal::{SignallingControl, SignallingEvent};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::Instrument;

const CHANNEL_LIMIT: usize = 10;

#[derive(Debug, Clone)]
pub enum SignalMessage {
    Offer(String),
    Answer(String),
    IceCandidate(String),
}

pub struct RemotePeerHandle {
    pub peer_id: PeerId,
    pub control: mpsc::Sender<PeerConnectionControl>,
}

struct ConnectionHandle {
    handle: RemotePeerHandle,
}

pub struct Peer {
    id: PeerId,
    config: PeerConfig,
    signal_control: mpsc::Sender<SignallingControl>,
    connections: Arc<Mutex<HashMap<PeerId, ConnectionHandle>>>,
    pending_connections: Arc<Mutex<HashMap<ConnectionId, PeerId>>>,
    event_tx: mpsc::Sender<PeerEvent>,
    tasks: tokio::task::JoinSet<anyhow::Result<()>>,
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
        let pending_connections = Arc::new(Mutex::new(HashMap::new()));
        let pending_connections_clone = pending_connections.clone();
        let connections = Arc::new(Mutex::new(HashMap::new()));
        let connections_clone = connections.clone();

        let mut peer = Self {
            id: id.clone(),
            config,
            signal_control: signal_control.clone(),
            connections,
            pending_connections,
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
                    pending_connections_clone,
                    connections_clone,
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
        if self.connections.lock().await.contains_key(&peer_id) {
            return Err(Error::AlreadyConnected);
        }

        self.signal_control
            .send(SignallingControl::RequestConnection(peer_id.clone()))
            .await
            .map_err(|_| Error::ConnectionFailed("Signal channel closed".into()))?;

        Ok(())
    }

    fn spawn_connection_event_handler(
        &mut self,
        peer_id: PeerId,
        control: mpsc::Sender<PeerConnectionControl>,
        mut events: mpsc::Receiver<PeerConnectionEvent>,
    ) {
        let event_tx = self.event_tx.clone();

        self.tasks.spawn(
            async move {
                tracing::info!("Connection event handler started for peer {}", peer_id);
                while let Some(event) = events.recv().await {
                    match event {
                        PeerConnectionEvent::StreamRequest(request) => {
                            tracing::info!("StreamRequest received for peer {}", peer_id);
                            let (response_tx, response_rx) = oneshot::channel();

                            let control_clone = control.clone();
                            tokio::spawn(async move {
                                if let Ok(response) = response_rx.await {
                                    let _ = control_clone
                                        .send(PeerConnectionControl::RequestStreamResponse(response))
                                        .await;
                                }
                            });

                            if event_tx
                                .send(PeerEvent::IncomingStream {
                                    peer_id: peer_id.clone(),
                                    request,
                                    response_tx,
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        PeerConnectionEvent::StreamResponse(response) => {
                            tracing::info!("StreamResponse received for peer {}", peer_id);
                            if event_tx
                                .send(PeerEvent::StreamResponse {
                                    peer_id: peer_id.clone(),
                                    response,
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        PeerConnectionEvent::Audio(_) | PeerConnectionEvent::Video(_) => {}
                        PeerConnectionEvent::Error(_) | PeerConnectionEvent::Closed => {
                            break;
                        }
                    }
                }

                anyhow::Ok(())
            }
            .in_current_span(),
        );
    }

    pub async fn accept_connection(&mut self, connection_id: ConnectionId) -> Result<()> {
        let Some(peer_id) = self.pending_connections.lock().await.remove(&connection_id) else {
            return Err(Error::ConnectionFailed("No pending connection".into()));
        };

        self.signal_control
            .send(SignallingControl::AcceptConnection(connection_id))
            .await
            .map_err(|_| Error::ConnectionFailed("Signal channel closed".into()))?;

        let (control, events) = create_peer_connection(
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
            control: control.clone(),
        };

        self.spawn_connection_event_handler(peer_id.clone(), control, events);

        self.connections.lock().await.insert(peer_id.clone(), ConnectionHandle { handle });

        self.event_tx
            .send(PeerEvent::PeerConnected { peer_id })
            .await
            .map_err(|_| Error::ConnectionFailed("Event channel closed".into()))?;

        Ok(())
    }

    pub async fn disconnect(&mut self, peer_id: &PeerId) -> Result<()> {
        if let Some(conn) = self.connections.lock().await.remove(peer_id) {
            let _ = conn.handle.control.send(PeerConnectionControl::Disconnect).await;
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
        let connections = self.connections.lock().await;
        let Some(conn) = connections.get(peer_id) else {
            return Err(Error::PeerNotFound(peer_id.clone()));
        };
        tracing::info!("Requesting stream from peer {}", peer_id);
        conn.handle
            .control
            .send(PeerConnectionControl::RequestStream(request))
            .await
            .map_err(|_| Error::ConnectionFailed("Control channel closed".into()))?;
        Ok(())
    }

    pub async fn connected_peers(&self) -> Vec<PeerId> {
        self.connections.lock().await.keys().cloned().collect()
    }

    async fn handle_signal_events(
        our_id: PeerId,
        config: PeerConfig,
        signal_control: mpsc::Sender<SignallingControl>,
        mut signal_event: mpsc::Receiver<SignallingEvent>,
        event_tx: mpsc::Sender<PeerEvent>,
        pending_connections: Arc<Mutex<HashMap<ConnectionId, PeerId>>>,
        connections: Arc<Mutex<HashMap<PeerId, ConnectionHandle>>>,
    ) -> anyhow::Result<()> {
        while let Some(event) = signal_event.recv().await {
            match event {
                SignallingEvent::Id(_) => {
                    tracing::warn!("received duplicate peer id");
                }
                SignallingEvent::ConectionRequest(peer_id, connection_id) => {
                    pending_connections.lock().await.insert(connection_id.clone(), peer_id.clone());
                    let _ = event_tx
                        .send(PeerEvent::ConnectionRequested { peer_id, connection_id })
                        .await;
                }
                SignallingEvent::ConnectionAccepted(peer_id, _connection_id) => {
                    let Ok((control, mut events)) = create_peer_connection(
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

                    let handle = RemotePeerHandle {
                        peer_id: peer_id.clone(),
                        control: control.clone(),
                    };

                    let event_tx_clone = event_tx.clone();
                    let peer_id_clone = peer_id.clone();
                    tokio::spawn(async move {
                        tracing::info!("Signalling connection event handler started for peer {}", peer_id_clone);
                        while let Some(event) = events.recv().await {
                            match event {
                                PeerConnectionEvent::StreamRequest(request) => {
                                    tracing::info!("StreamRequest received for peer {} (from signalling)", peer_id_clone);
                                    let (response_tx, response_rx) = oneshot::channel();

                                    let control_clone = control.clone();
                                    tokio::spawn(async move {
                                        if let Ok(response) = response_rx.await {
                                            let _ = control_clone
                                                .send(PeerConnectionControl::RequestStreamResponse(response))
                                                .await;
                                        }
                                    });

                                    if event_tx_clone
                                        .send(PeerEvent::IncomingStream {
                                            peer_id: peer_id_clone.clone(),
                                            request,
                                            response_tx,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                PeerConnectionEvent::StreamResponse(response) => {
                                    tracing::info!("StreamResponse received for peer {} (from signalling)", peer_id_clone);
                                    if event_tx_clone
                                        .send(PeerEvent::StreamResponse {
                                            peer_id: peer_id_clone.clone(),
                                            response,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                PeerConnectionEvent::Audio(_) | PeerConnectionEvent::Video(_) => {}
                                PeerConnectionEvent::Error(_) | PeerConnectionEvent::Closed => {
                                    break;
                                }
                            }
                        }
                        anyhow::Ok(())
                    });

                    connections.lock().await.insert(
                        peer_id.clone(),
                        ConnectionHandle { handle },
                    );

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
