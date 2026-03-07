use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use remote::{
    Peer, PeerConfig, PeerEvent, PeerId, StreamRequest, StreamRequestResponse,
};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::event::{AppCommand, AppEvent};

const CHANNEL_SIZE: usize = 128;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

struct StreamResponseHandle {
    response_tx: oneshot::Sender<StreamRequestResponse>,
}

pub fn spawn_peer_actor(
    runtime: &tokio::runtime::Runtime,
    config: PeerConfig,
    event_tx: mpsc::Sender<AppEvent>,
) {
    runtime.spawn(async move {
        match Peer::new(config).await {
            Ok((peer, mut peer_events)) => {
                let local_id = peer.id().clone();
                run_peer_actor(peer, local_id, &mut peer_events, event_tx).await;
            }
            Err(e) => {
                let error = e.to_string();
                tracing::error!("Failed to create peer: {}", e);
                let _ = event_tx.send(AppEvent::PeerCreationFailed { error }).await;
            }
        }
    });
}

async fn run_peer_actor(
    mut peer: Peer,
    local_id: PeerId,
    peer_events: &mut mpsc::Receiver<PeerEvent>,
    event_tx: mpsc::Sender<AppEvent>,
) {
    let (command_tx, mut command_rx) = mpsc::channel::<AppCommand>(CHANNEL_SIZE);
    let pending_responses: Arc<Mutex<HashMap<u64, StreamResponseHandle>>> =
        Arc::new(Mutex::new(HashMap::new()));

    if let Err(e) = event_tx
        .send(AppEvent::PeerCreated {
            local_id: local_id.clone(),
            command_tx,
        })
        .await
    {
        tracing::error!("Failed to send PeerCreated event: {}", e);
        return;
    }

    tracing::info!("Peer actor started for {}", local_id);

    loop {
        tokio::select! {
            cmd = command_rx.recv() => {
                let Some(cmd) = cmd else {
                    tracing::debug!("Command channel closed for peer {}", local_id);
                    break;
                };

                handle_command(cmd, &mut peer, &local_id, pending_responses.clone()).await;
            }

            event = peer_events.recv() => {
                let Some(event) = event else {
                    tracing::debug!("Peer {} event channel closed", local_id);
                    let _ = event_tx.send(AppEvent::PeerDestroyed { local_id: local_id.clone() }).await;
                    break;
                };

                if let Err(e) = handle_peer_event(event, &local_id, &event_tx, pending_responses.clone()).await {
                    tracing::error!("Failed to handle peer event: {}", e);
                }
            }
        }
    }

    tracing::info!("Peer {} actor shut down", local_id);
}

async fn handle_peer_event(
    event: PeerEvent,
    local_id: &PeerId,
    event_tx: &mpsc::Sender<AppEvent>,
    pending_responses: Arc<Mutex<HashMap<u64, StreamResponseHandle>>>,
) -> Result<(), mpsc::error::SendError<AppEvent>> {
    match event {
        PeerEvent::ConnectionRequested { peer_id, connection_id } => {
            event_tx
                .send(AppEvent::ConnectionRequested {
                    local_id: local_id.clone(),
                    peer_id,
                    connection_id,
                })
                .await
        }
        PeerEvent::PeerConnected { peer_id } => {
            event_tx
                .send(AppEvent::PeerConnected {
                    local_id: local_id.clone(),
                    peer_id,
                })
                .await
        }
        PeerEvent::PeerDisconnected { peer_id, reason: _ } => {
            event_tx
                .send(AppEvent::PeerDisconnected {
                    local_id: local_id.clone(),
                    peer_id,
                })
                .await
        }
        PeerEvent::SignalMessage { .. } => Ok(()),
        PeerEvent::IncomingStream {
            peer_id,
            request,
            response_tx,
        } => {
            let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
            pending_responses.lock().await.insert(
                request_id,
                StreamResponseHandle { response_tx },
            );
            event_tx
                .send(AppEvent::StreamRequestReceived {
                    local_id: local_id.clone(),
                    peer_id,
                    request,
                    request_id,
                })
                .await
        }
        PeerEvent::StreamResponse { peer_id, response } => {
            event_tx
                .send(AppEvent::StreamResponseReceived {
                    local_id: local_id.clone(),
                    peer_id,
                    response,
                })
                .await
        }
        PeerEvent::Error { peer_id: _, error } => {
            event_tx
                .send(AppEvent::PeerError {
                    local_id: local_id.clone(),
                    error,
                })
                .await
        }
    }
}

async fn handle_command(
    cmd: AppCommand,
    peer: &mut Peer,
    local_id: &PeerId,
    pending_responses: Arc<Mutex<HashMap<u64, StreamResponseHandle>>>,
) {
    match cmd {
        AppCommand::ConnectToPeer { peer_id } => {
            let peer_id_str = peer_id.to_string();
            if let Err(e) = peer.connect(peer_id).await {
                tracing::error!("Peer {} failed to connect to {}: {}", local_id, peer_id_str, e);
            }
        }

        AppCommand::AcceptConnection { peer_id, connection_id } => {
            if let Err(e) = peer.accept_connection(connection_id).await {
                tracing::error!(
                    "Peer {} failed to accept connection from {}: {}",
                    local_id, peer_id, e
                );
            }
        }

        AppCommand::Disconnect { peer_id } => {
            if let Err(e) = peer.disconnect(&peer_id).await {
                tracing::error!("Peer {} failed to disconnect from {}: {}", local_id, peer_id, e);
            }
        }

        AppCommand::RequestStream { peer_id, request } => {
            if let Err(e) = peer.request_stream(&peer_id, request).await {
                tracing::error!("Peer {} failed to request stream from {}: {}", local_id, peer_id, e);
            }
        }

        AppCommand::RespondToStreamRequest { request_id, response } => {
            let Some(handle) = pending_responses.lock().await.remove(&request_id) else {
                tracing::warn!("No pending stream request with id {}", request_id);
                return;
            };
            if let Err(_) = handle.response_tx.send(response) {
                tracing::warn!("Failed to send stream response for request {}", request_id);
            }
        }

        AppCommand::Shutdown => {
            tracing::info!("Peer {} shutdown requested", local_id);
        }
    }
}
