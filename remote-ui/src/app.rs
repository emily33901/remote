use std::sync::Arc;
use std::time::Instant;

use remote::{Peer, PeerEvent, PeerId};
use tokio::sync::Mutex;

use crate::config::Config;

pub struct RemoteApp {
    peer: Option<Arc<Mutex<Option<Peer>>>>,
    peer_events: Option<Arc<Mutex<Option<tokio::sync::mpsc::Receiver<PeerEvent>>>>>,
    connect_input: String,
    start_time: Instant,
}

impl RemoteApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            peer: None,
            peer_events: None,
            connect_input: String::new(),
            start_time: Instant::now(),
        }
    }
}

impl eframe::App for RemoteApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_events();

        egui::Window::new("Clock").show(ctx, |ui| {
            let elapsed = self.start_time.elapsed();
            ui.label(
                egui::RichText::new(format!(
                    "{:8}:{:04}",
                    elapsed.as_secs(),
                    elapsed.subsec_millis()
                ))
                .font(egui::FontId::monospace(30.0))
                .size(50.0),
            );
        });

        egui::Window::new("Remote").show(ctx, |ui| {
            if self.peer.is_none() {
                if ui.button("Create Peer").clicked() {
                    let config = Config::load().peer_config.clone();
                    let peer = Arc::new(Mutex::new(None));
                    let events = Arc::new(Mutex::new(None));
                    self.peer = Some(peer.clone());
                    self.peer_events = Some(events.clone());

                    tokio::spawn(async move {
                        match Peer::new(config).await {
                            Ok((p, e)) => {
                                *peer.lock().await = Some(p);
                                *events.lock().await = Some(e);
                                tracing::info!("Peer created");
                            }
                            Err(e) => {
                                tracing::error!("Failed to create peer: {}", e);
                            }
                        };
                    });
                }
            } else {
                self.show_peer_ui(ui);
            }
        });
    }
}

impl RemoteApp {
    fn poll_events(&mut self) {
        let Some(events_arc) = self.peer_events.clone() else {
            return;
        };

        tokio::spawn(async move {
            let mut events_opt = events_arc.lock().await;
            let Some(ref mut events) = events_opt.as_mut() else {
                return;
            };

            while let Ok(event) = events.try_recv() {
                match event {
                    PeerEvent::ConnectionRequested { peer_id, .. } => {
                        tracing::info!("Connection requested from: {}", peer_id);
                    }
                    PeerEvent::PeerConnected { peer_id } => {
                        tracing::info!("Peer connected: {}", peer_id);
                    }
                    PeerEvent::PeerDisconnected { peer_id, reason } => {
                        tracing::info!("Peer disconnected: {} ({:?})", peer_id, reason);
                    }
                    PeerEvent::SignalMessage { peer_id, message } => {
                        tracing::debug!("Signal from {}: {:?}", peer_id, message);
                    }
                    PeerEvent::IncomingStream { peer_id } => {
                        tracing::info!("Stream from {}", peer_id);
                    }
                    PeerEvent::Error { peer_id, error } => {
                        tracing::error!("Error from {:?}: {}", peer_id, error);
                    }
                }
            }
        });
    }

    fn show_peer_ui(&mut self, ui: &mut egui::Ui) {
        let Some(peer_arc) = self.peer.clone() else {
            return;
        };

        let Ok(mut peer_guard) = peer_arc.try_lock() else {
            ui.label("Peer is busy...");
            return;
        };
        let Some(ref peer) = peer_guard.as_ref() else {
            ui.label("Peer is initializing...");
            return;
        };

        ui.horizontal(|ui| {
            ui.label(format!("Peer ID: {}", peer.id()));
        });
        ui.separator();

        ui.heading("Connect");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.connect_input);
            if ui.button("Connect").clicked() && !self.connect_input.is_empty() {
                let peer_id: PeerId = self.connect_input.clone().into();
                let peer_arc = peer_arc.clone();

                tokio::spawn(async move {
                    let mut guard = peer_arc.lock().await;
                    if let Some(ref mut peer) = guard.as_mut() {
                        if let Err(e) = peer.connect(peer_id).await {
                            tracing::error!("Failed to connect: {}", e);
                        }
                    }
                });
                self.connect_input.clear();
            }
        });

        ui.separator();
        ui.label("Events will appear in the console");
    }
}
