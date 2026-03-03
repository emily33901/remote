mod audio;
mod chunk;
mod config;
mod ext;
mod input;
mod logic;
mod peer;
mod player;
mod presenter;
mod ui;
mod video;
mod windows;

use crate::config::Config;
use tracing_subscriber::Layer;

use clap::Parser;
use rtc;
use signal::PeerId;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const ARBITRARY_CHANNEL_LIMIT: usize = 5;

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Make sure we can load the dotenv and create a config from it.
    dotenv::dotenv().expect("Failed to load dotenv");

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(LevelFilter::DEBUG.into())
        .from_env()
        .expect("Failed to create filter")
        .add_directive("webrtc_sctp::association=info".parse().unwrap())
        .add_directive(
            "webrtc_sctp::association::association_internal=info"
                .parse()
                .unwrap(),
        )
        .add_directive("webrtc_sctp::stream=info".parse().unwrap());

    tracing_subscriber::registry()
        .with(console_subscriber::spawn())
        .with(
            tracing_subscriber::fmt::layer()
                .compact()
                .with_filter(filter),
        )
        .init();

    let _system = windows::System::new().expect("Failed to initialize Windows system");
    let config = Config::load();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(egui::Vec2::new(config.width as f32, config.height as f32)),
        ..Default::default()
    };

    eframe::run_native(
        "remote",
        options,
        Box::new(|cc| Ok(Box::new(ui::app::App::new(cc)))),
    )
    .expect("Failed to run eframe");
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(default_value = "ui")]
    command: String,
}
