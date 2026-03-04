use std::{collections::HashMap, time::Instant};

use tokio::sync::mpsc;

use crate::config::Config;
use signal::{ConnectionId, PeerId};

use super::peer::{ConnectedPeer, PeerWindowState, ShouldRemove, UIPeer};

pub enum AppEvent {
    Peer(UIPeer),
    ConnectionRequest(PeerId, (ConnectionId, PeerId)),
    RemotePeerConnected(PeerId, (PeerId, mpsc::Sender<crate::peer::PeerControl>)),
    RemotePeerStreamRequest(
        PeerId,
        (
            PeerId,
            crate::logic::PeerStreamRequest,
            tokio::sync::oneshot::Sender<(
                crate::logic::PeerStreamRequestResponse,
                Option<media::encoder::Encoder>,
            )>,
        ),
    ),
    DecoderEvent(
        PeerId,
        (PeerId, mpsc::Receiver<media::decoder::DecoderEvent>),
    ),
    PeerClosed(PeerId, PeerId),
    VideoData(PeerId, PeerId, Vec<u8>),
}

pub struct App {
    pub peers: HashMap<PeerId, (PeerWindowState, UIPeer)>,
    pub event_rx: mpsc::Receiver<AppEvent>,
    pub event_tx: mpsc::Sender<AppEvent>,
    pub start_time: std::time::Instant,
    pub gl: std::sync::Arc<glow::Context>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App").field("peers", &self.peers).finish()
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let gl = cc.gl.clone().expect("Failed to get OpenGL context");

        let (event_tx, event_rx) = mpsc::channel(10);

        Self {
            peers: Default::default(),
            event_rx,
            event_tx,
            start_time: Instant::now(),
            gl,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}

impl App {
    #[tracing::instrument(skip(self, ctx))]
    pub fn ui(&mut self, ctx: &egui::Context) {
        let wallclock = egui::Window::new("Clock");

        wallclock.show(ctx, |ui| {
            let elapsed = self.start_time.elapsed();
            ui.label(
                egui::RichText::new(format!(
                    "{:8}:{:04}",
                    elapsed.as_secs(),
                    elapsed.subsec_millis()
                ))
                .font(egui::FontId::monospace(30.0))
                .size(50.0),
            )
        });

        let ui = egui::Window::new("App");

        ui.show(ctx, |ui| {
            if ui.button("new peer").clicked() {
                tokio::spawn({
                    let event_tx = self.event_tx.clone();
                    async move {
                        let config = Config::load();
                        let peer = UIPeer::new(&config.signal_server, event_tx.clone())
                            .await
                            .unwrap();
                        event_tx.send(AppEvent::Peer(peer)).await.unwrap();
                    }
                });
            }

            ui.heading("Peers");

            let mut remove_peers = vec![];

            {
                for (id, (window_state, peer)) in self.peers.iter_mut() {
                    match window_state.ui(ctx, ui, peer, &self.gl) {
                        ShouldRemove::Yes => {
                            remove_peers.push(id.clone());
                        }
                        ShouldRemove::No => {}
                    }
                }
            }

            for peer in remove_peers {
                self.peers.remove(&peer);
            }

            if let Ok(event) = self.event_rx.try_recv() {
                match event {
                    AppEvent::Peer(peer) => {
                        self.peers
                            .insert(peer.our_id().clone(), (PeerWindowState::default(), peer));
                    }
                    AppEvent::ConnectionRequest(our_peer_id, (connection_request_id, peer_id)) => {
                        if let Some((peer_window_state, _)) = self.peers.get_mut(&our_peer_id) {
                            peer_window_state
                                .connection_requests
                                .insert(connection_request_id, peer_id);
                        }
                    }
                    AppEvent::RemotePeerConnected(peer_id, (their_peer_id, control)) => {
                        if let Some((peer_window_state, _)) = self.peers.get_mut(&peer_id) {
                            peer_window_state
                                .connected_peers
                                .insert(their_peer_id.clone(), ConnectedPeer::new(control));
                        }

                        if let Some((peer_window_state, _)) = self.peers.get_mut(&peer_id) {
                            if let Some(connection_id) = (|| {
                                for (c_id, p_id) in &peer_window_state.connection_requests {
                                    if p_id == &their_peer_id {
                                        return Some(c_id.clone());
                                    }
                                }

                                None
                            })() {
                                peer_window_state.connection_requests.remove(&connection_id);
                            }
                        }
                    }
                    AppEvent::RemotePeerStreamRequest(our_id, (their_id, request, response_tx)) => {
                        if let Some(connected_peer) =
                            self.peers
                                .get_mut(&our_id)
                                .and_then(|(peer_window_state, _)| {
                                    peer_window_state.connected_peers.get_mut(&their_id)
                                })
                        {
                            connected_peer.stream_requests.push((request, response_tx));
                        }
                    }

                    AppEvent::DecoderEvent(our_id, (their_id, decoder_event)) => {
                        if let Some(connected_peer) =
                            self.peers
                                .get_mut(&our_id)
                                .and_then(|(peer_window_state, _)| {
                                    peer_window_state.connected_peers.get_mut(&their_id)
                                })
                        {
                            connected_peer.decoder_receiver = Some(decoder_event);
                        }
                    }
                    AppEvent::PeerClosed(our_id, their_id) => {
                        if let Some((peer_window_state, _)) = self.peers.get_mut(&our_id) {
                            peer_window_state
                                .connected_peers
                                .remove(&their_id)
                                .expect("Expect remote PeerControl to exist when it goes away");
                        }
                    }
                    AppEvent::VideoData(our_id, their_id, data) => {
                        if let Some((peer_window_state, _)) = self.peers.get_mut(&our_id) {
                            if let Some(connected_peer) =
                                peer_window_state.connected_peers.get_mut(&their_id)
                            {
                                if let Ok(mut guard) = connected_peer.latest_h264_data.lock() {
                                    *guard = Some(data.clone());
                                }
                            }
                        }
                    }
                }
            }
        });
    }
}
