use anyhow::Result;
use remote::{Peer, PeerConfig, PeerEvent};
use tokio::sync::mpsc;

use crate::event::{AppCommand, AppEvent};

const CHANNEL_SIZE: usize = 128;

pub struct PeerActorHandle {
    pub command_tx: mpsc::Sender<AppCommand>,
    pub event_rx: mpsc::Receiver<AppEvent>,
}

pub fn spawn_peer_actor(runtime: &tokio::runtime::Runtime, config: PeerConfig) -> PeerActorHandle {
    let (command_tx, command_rx) = mpsc::channel(CHANNEL_SIZE);
    let (event_tx, event_rx) = mpsc::channel(CHANNEL_SIZE);

    runtime.spawn(run_peer_actor(config, command_rx, event_tx));

    PeerActorHandle { command_tx, event_rx }
}

async fn run_peer_actor(
    config: PeerConfig,
    mut command_rx: mpsc::Receiver<AppCommand>,
    event_tx: mpsc::Sender<AppEvent>,
) {
    let mut peer: Option<Peer> = None;
    let mut peer_events: Option<mpsc::Receiver<PeerEvent>> = None;

    loop {
        tokio::select! {
            cmd = command_rx.recv() => {
                let Some(cmd) = cmd else {
                    tracing::debug!("Command channel closed, actor exiting");
                    break;
                };

                match handle_command(cmd, &mut peer, &config, &event_tx).await {
                    Ok(new_peer_events) => {
                        peer_events = new_peer_events.or(peer_events);
                    }
                    Err(e) => {
                        tracing::error!("Command handling error: {}", e);
                    }
                }
            }

            event = async {
                if let Some(rx) = peer_events.as_mut() {
                    rx.recv().await
                } else {
                    std::future::pending().await
                }
            } => {
                let Some(event) = event else {
                    tracing::debug!("Peer event channel closed");
                    peer = None;
                    peer_events = None;
                    let _ = event_tx.send(AppEvent::PeerDestroyed).await;
                    continue;
                };

                if let Err(e) = event_tx.send(AppEvent::PeerEvent(event)).await {
                    tracing::error!("Failed to forward peer event: {}", e);
                    break;
                }
            }
        }
    }

    tracing::info!("Peer actor shut down");
}

async fn handle_command(
    cmd: AppCommand,
    peer: &mut Option<Peer>,
    config: &PeerConfig,
    event_tx: &mpsc::Sender<AppEvent>,
) -> Result<Option<mpsc::Receiver<PeerEvent>>> {
    match cmd {
        AppCommand::CreatePeer => {
            if peer.is_some() {
                tracing::warn!("Peer already exists, ignoring CreatePeer");
                return Ok(None);
            }

            match Peer::new(config.clone()).await {
                Ok((p, events)) => {
                    let local_id = p.id().clone();
                    *peer = Some(p);
                    let _ = event_tx.send(AppEvent::PeerCreated { local_id: local_id.clone() }).await;
                    tracing::info!("Peer created with id: {}", local_id);
                    Ok(Some(events))
                }
                Err(e) => {
                    let error = e.to_string();
                    tracing::error!("Failed to create peer: {}", e);
                    let _ = event_tx.send(AppEvent::PeerCreationFailed { error }).await;
                    Ok(None)
                }
            }
        }

        AppCommand::ConnectToPeer { peer_id } => {
            let Some(p) = peer.as_mut() else {
                tracing::warn!("Cannot connect: no peer exists");
                return Ok(None);
            };

            let peer_id_str = peer_id.to_string();
            if let Err(e) = p.connect(peer_id).await {
                tracing::error!("Failed to connect to {}: {}", peer_id_str, e);
            }
            Ok(None)
        }

        AppCommand::AcceptConnection { peer_id, connection_id } => {
            let Some(p) = peer.as_mut() else {
                tracing::warn!("Cannot accept: no peer exists");
                return Ok(None);
            };

            if let Err(e) = p.accept_connection(connection_id).await {
                tracing::error!("Failed to accept connection from {}: {}", peer_id, e);
            }
            Ok(None)
        }

        AppCommand::Disconnect { peer_id } => {
            let Some(p) = peer.as_mut() else {
                tracing::warn!("Cannot disconnect: no peer exists");
                return Ok(None);
            };

            if let Err(e) = p.disconnect(&peer_id).await {
                tracing::error!("Failed to disconnect from {}: {}", peer_id, e);
            }
            Ok(None)
        }

        AppCommand::Shutdown => {
            *peer = None;
            tracing::info!("Peer shut down via command");
            Ok(None)
        }
    }
}
