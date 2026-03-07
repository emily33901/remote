use remote::{Peer, PeerConfig, PeerEvent, PeerId};
use tokio::sync::mpsc;

use crate::event::{AppCommand, AppEvent};

const CHANNEL_SIZE: usize = 128;

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
    
    if let Err(e) = event_tx.send(AppEvent::PeerCreated {
        local_id: local_id.clone(),
        command_tx,
    }).await {
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

                handle_command(cmd, &mut peer, &local_id).await;
            }

            event = peer_events.recv() => {
                let Some(event) = event else {
                    tracing::debug!("Peer {} event channel closed", local_id);
                    let _ = event_tx.send(AppEvent::PeerDestroyed { local_id: local_id.clone() }).await;
                    break;
                };

                if let Err(e) = event_tx.send(AppEvent::PeerEvent {
                    local_id: local_id.clone(),
                    event,
                }).await {
                    tracing::error!("Failed to forward peer event: {}", e);
                    break;
                }
            }
        }
    }

    tracing::info!("Peer {} actor shut down", local_id);
}

async fn handle_command(
    cmd: AppCommand,
    peer: &mut Peer,
    local_id: &PeerId,
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
                tracing::error!("Peer {} failed to accept connection from {}: {}", local_id, peer_id, e);
            }
        }

        AppCommand::Disconnect { peer_id } => {
            if let Err(e) = peer.disconnect(&peer_id).await {
                tracing::error!("Peer {} failed to disconnect from {}: {}", local_id, peer_id, e);
            }
        }

        AppCommand::Shutdown => {
            tracing::info!("Peer {} shutdown requested", local_id);
        }
    }
}
