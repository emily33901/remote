mod actor;
mod config;
mod event;
mod state;

use std::collections::HashMap;
use std::time::Instant;

use actor::spawn_peer_actor;
use anyhow::Result;
use config::Config;
use event::{AppCommand, AppEvent};
use remote::{PeerConfig, PeerId};
use state::{PeerHandle, RemoteUIState};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

fn main() {
    let _ = dotenv::dotenv();

    let filter = EnvFilter::builder()
        .with_default_directive(tracing::level_filters::LevelFilter::DEBUG.into())
        .from_env()
        .unwrap_or_default()
        .add_directive("webrtc_sctp::association=info".parse().unwrap())
        .add_directive("webrtc_sctp::stream=info".parse().unwrap());

    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(filter)
        .init();

    let config = config::Config::load();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(egui::Vec2::new(config.width as f32, config.height as f32)),
        ..Default::default()
    };

    eframe::run_native(
        "remote",
        options,
        Box::new(|cc| Ok(Box::new(RemoteAppUI::new(cc)?))),
    )
    .expect("Failed to run eframe");
}

pub struct RemoteAppUI {
    state: RemoteUIState,
    peer_handles: HashMap<PeerId, PeerHandle>,
    event_rx: mpsc::Receiver<AppEvent>,
    event_tx: mpsc::Sender<AppEvent>,
    runtime: tokio::runtime::Runtime,
    config: PeerConfig,
    connect_input: String,
    selected_peer: Option<PeerId>,
    start_time: Instant,
}

impl RemoteAppUI {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let (event_tx, event_rx) = mpsc::channel(128);
        let config = Config::load().peer_config.clone();

        Ok(Self {
            state: RemoteUIState::default(),
            peer_handles: HashMap::new(),
            event_rx,
            event_tx,
            runtime,
            config,
            connect_input: String::new(),
            selected_peer: None,
            start_time: Instant::now(),
        })
    }

    fn poll_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AppEvent::PeerCreated { local_id, command_tx } => {
                    tracing::info!("Peer created with id: {}", local_id);
                    self.state.add_local_peer(local_id.clone());
                    self.peer_handles.insert(
                        local_id.clone(),
                        PeerHandle { command_tx },
                    );
                    if self.selected_peer.is_none() {
                        self.selected_peer = Some(local_id);
                    }
                }
                AppEvent::PeerEvent { local_id, event } => {
                    self.state.apply_peer_event(&local_id, event);
                }
                AppEvent::PeerCreationFailed { error } => {
                    tracing::error!("Peer creation failed: {}", error);
                    self.state.last_error = Some(error);
                }
                AppEvent::PeerDestroyed { local_id } => {
                    tracing::info!("Peer {} destroyed", local_id);
                    self.state.remove_local_peer(&local_id);
                    self.peer_handles.remove(&local_id);
                    if self.selected_peer == Some(local_id) {
                        self.selected_peer = self.peer_handles.keys().next().cloned();
                    }
                }
            }
        }
    }

    fn create_peer(&self) {
        spawn_peer_actor(&self.runtime, self.config.clone(), self.event_tx.clone());
    }

    fn send_command(&self, local_id: &PeerId, cmd: AppCommand) {
        if let Some(handle) = self.peer_handles.get(local_id) {
            if let Err(e) = handle.command_tx.blocking_send(cmd) {
                tracing::error!("Failed to send command to peer {}: {}", local_id, e);
            }
        }
    }
}

impl eframe::App for RemoteAppUI {
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
            ui.horizontal(|ui| {
                if ui.button("Create Peer").clicked() {
                    self.create_peer();
                }
            });

            if let Some(error) = &self.state.last_error {
                ui.colored_label(egui::Color32::RED, error);
            }

            if !self.peer_handles.is_empty() {
                ui.separator();
                self.show_peer_selector(ui);
                
                if let Some(selected) = self.selected_peer.clone() {
                    self.show_peer_ui(ui, &selected);
                }
            }
        });
    }
}

impl RemoteAppUI {
    fn show_peer_selector(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Local Peers:");
            let peer_ids: Vec<PeerId> = self.peer_handles.keys().cloned().collect();
            for peer_id in peer_ids {
                let is_selected = self.selected_peer.as_ref() == Some(&peer_id);
                if ui.selectable_label(is_selected, peer_id.to_string()).clicked() {
                    self.selected_peer = Some(peer_id);
                }
            }
        });
        ui.separator();
    }

    fn show_peer_ui(&mut self, ui: &mut egui::Ui, local_id: &PeerId) {
        ui.horizontal(|ui| {
            ui.label(format!("Peer ID: {}", local_id));
            if ui.button("Shutdown").clicked() {
                self.send_command(local_id, AppCommand::Shutdown);
            }
        });

        ui.separator();

        ui.heading("Connect");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.connect_input);
            if ui.button("Connect").clicked() && !self.connect_input.is_empty() {
                let peer_id: PeerId = self.connect_input.clone().into();
                self.send_command(local_id, AppCommand::ConnectToPeer { peer_id });
                self.connect_input.clear();
            }
        });

        if let Some(peer_state) = self.state.local_peers.get(local_id) {
            if !peer_state.connected_peers.is_empty() {
                ui.separator();
                ui.heading("Connected Peers");
                for peer_id in peer_state.connected_peers.clone() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{}", peer_id));
                        if ui.button("Disconnect").clicked() {
                            self.send_command(local_id, AppCommand::Disconnect { peer_id });
                        }
                    });
                }
            }

            if !peer_state.pending_connections.is_empty() {
                ui.separator();
                ui.heading("Pending Connections");
                for pending in peer_state.pending_connections.clone() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} wants to connect", pending.peer_id));
                        if ui.button("Accept").clicked() {
                            self.send_command(local_id, AppCommand::AcceptConnection {
                                peer_id: pending.peer_id,
                                connection_id: pending.connection_id,
                            });
                        }
                    });
                }
            }
        }
    }
}
