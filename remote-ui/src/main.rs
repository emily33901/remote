mod actor;
mod config;
mod event;
mod state;

use std::time::Instant;

use actor::spawn_peer_actor;
use anyhow::Result;
use event::{AppCommand, AppEvent};
use remote::PeerId;
use state::{RemoteUIState, UIState};
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
    command_tx: mpsc::Sender<AppCommand>,
    event_rx: mpsc::Receiver<AppEvent>,
    runtime: tokio::runtime::Runtime,
    connect_input: String,
    start_time: Instant,
}

impl RemoteAppUI {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let config = config::Config::load().peer_config.clone();
        let handle = spawn_peer_actor(&runtime, config);

        Ok(Self {
            state: RemoteUIState::default(),
            command_tx: handle.command_tx,
            event_rx: handle.event_rx,
            runtime,
            connect_input: String::new(),
            start_time: Instant::now(),
        })
    }

    fn poll_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AppEvent::PeerCreated { local_id } => {
                    tracing::info!("Peer created with id: {}", local_id);
                    self.state.local_peer_id = Some(local_id);
                    self.state.state = UIState::Connected;
                }
                AppEvent::PeerEvent(e) => {
                    self.state.apply_peer_event(e);
                }
                AppEvent::PeerCreationFailed { error } => {
                    tracing::error!("Peer creation failed: {}", error);
                    self.state.state = UIState::Error(error);
                }
                AppEvent::PeerDestroyed => {
                    tracing::info!("Peer destroyed");
                    self.state = RemoteUIState::default();
                }
            }
        }
    }

    fn send_command(&self, cmd: AppCommand) {
        if let Err(e) = self.command_tx.blocking_send(cmd) {
            tracing::error!("Failed to send command: {}", e);
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
            match &self.state.state {
                UIState::Disconnected | UIState::Error(_) => {
                    if ui.button("Create Peer").clicked() {
                        self.send_command(AppCommand::CreatePeer);
                    }
                    if let UIState::Error(e) = &self.state.state {
                        ui.colored_label(egui::Color32::RED, e);
                    }
                }
                UIState::Connecting => {
                    ui.label("Connecting...");
                }
                UIState::Connected => {
                    self.show_peer_ui(ui);
                }
            }
        });
    }
}

impl RemoteAppUI {
    fn show_peer_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(id) = &self.state.local_peer_id {
            ui.horizontal(|ui| {
                ui.label(format!("Peer ID: {}", id));
            });
        }
        ui.separator();

        ui.heading("Connect");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.connect_input);
            if ui.button("Connect").clicked() && !self.connect_input.is_empty() {
                let peer_id: PeerId = self.connect_input.clone().into();
                self.send_command(AppCommand::ConnectToPeer { peer_id });
                self.connect_input.clear();
            }
        });

        if !self.state.connected_peers.is_empty() {
            ui.separator();
            ui.heading("Connected Peers");
            for peer_id in &self.state.connected_peers.clone() {
                ui.horizontal(|ui| {
                    ui.label(format!("{}", peer_id));
                    if ui.button("Disconnect").clicked() {
                        self.send_command(AppCommand::Disconnect { peer_id: peer_id.clone() });
                    }
                });
            }
        }

        if !self.state.pending_connections.is_empty() {
            ui.separator();
            ui.heading("Pending Connections");
            for pending in &self.state.pending_connections.clone() {
                ui.horizontal(|ui| {
                    ui.label(format!("{} wants to connect", pending.peer_id));
                    if ui.button("Accept").clicked() {
                        self.send_command(AppCommand::AcceptConnection {
                            peer_id: pending.peer_id.clone(),
                            connection_id: pending.connection_id.clone(),
                        });
                    }
                });
            }
        }
    }
}
