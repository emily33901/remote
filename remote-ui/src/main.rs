mod app;
mod config;

use std::time::Instant;

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
        Box::new(|cc| Ok(Box::new(app::RemoteApp::new(cc)))),
    )
    .expect("Failed to run eframe");
}
