mod audio;
mod chunk;
mod config;
mod ext;
mod input;
mod logic;
mod peer;
mod player;
mod ui;
mod video;
mod windows;

use crate::config::Config;
use std::str::FromStr;
use std::sync::Arc;
use std::{collections::HashMap, fmt::Display};

use clap::Parser;
use eyre::Result;
use media::{Encoding, EncodingOptions, H264EncodingOptions};
use peer::PeerControl;
use rtc;
use signal::SignallingControl;
use signal::{ConnectionId, PeerId};

use tokio::sync::{mpsc, Mutex};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use uuid::Uuid;

const ARBITRARY_CHANNEL_LIMIT: usize = 5;

#[derive(Debug, Clone)]
enum Command {
    Ui,
}

#[derive(Debug)]
struct CommandParseError;

impl Display for CommandParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CommandParseError")?;
        Ok(())
    }
}

impl std::error::Error for CommandParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }

    fn description(&self) -> &str {
        "description() is deprecated; use Display"
    }

    fn cause(&self) -> Option<&dyn std::error::Error> {
        self.source()
    }
}

impl FromStr for Command {
    type Err = CommandParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ui" => Ok(Self::Ui),
            _ => Err(CommandParseError),
        }
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    command: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Make sure we can load the dotenv and create a config from it.
    dotenv::dotenv()?;
    let config = Config::load();

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(LevelFilter::DEBUG.into())
        .from_env()?
        // .add_directive("tokio=trace".parse()?)
        // .add_directive("runtime=trace".parse()?)
        .add_directive("webrtc_sctp::association=info".parse()?)
        .add_directive("webrtc_sctp::association::association_internal=info".parse()?)
        .add_directive("webrtc_sctp::stream=info".parse()?);

    tracing_subscriber::registry()
        // .with(console_subscriber::spawn())
        // .with(tracing_tracy::TracyLayer::default())
        .with(filter)
        .with(tracing_subscriber::fmt::layer().pretty())
        .init();

    tracing::info!(args.command, config.signal_server, "remote");

    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::capture();
        eprintln!("thread panicked {info}");
        eprintln!("backtrace\n{backtrace}");
    }));

    let command = args.command.as_str().parse()?;
    match command {
        Command::Ui => Ok(ui::ui().await?),
    }
}
