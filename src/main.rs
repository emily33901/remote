mod audio;
mod channel;
mod chunk;
mod logic;
mod media;
mod peer;
mod player;
mod signalling;
mod util;
mod video;

use std::str::FromStr;
use std::sync::Arc;
use std::{collections::HashMap, fmt::Display};

use clap::{Arg, Parser};
use eyre::Result;
use peer::PeerControl;
use signalling::SignallingControl;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

pub(crate) type PeerId = Uuid;
pub(crate) type ConnectionId = Uuid;

#[derive(Debug, Clone)]
enum Command {
    SignallingServer,
    Peer,
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

    fn from_str(s: &str) -> std::prelude::v1::Result<Self, Self::Err> {
        match s {
            "server" => Ok(Self::SignallingServer),
            "peer" => Ok(Self::Peer),
            _ => Err(CommandParseError),
        }
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    address: String,
    #[arg(short, long)]
    name: String,
    command: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Configure logger at runtime
    let args = Args::parse();
    fern::Dispatch::new()
        // Perform allocation-free log formatting
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}] [{}] {}",
                record.level(),
                record.target(),
                message
            ))
        })
        // Add blanket level filter -
        .level(log::LevelFilter::Debug)
        .level_for("remote", log::LevelFilter::Debug)
        .level_for("webrtc_sctp::association", log::LevelFilter::Info)
        .level_for("webrtc_sctp::stream", log::LevelFilter::Info)
        .level_for("webrtc_sctp", log::LevelFilter::Info)
        .level_for(
            "webrtc_sctp::association::association_internal",
            log::LevelFilter::Info,
        )
        // - and per-module overrides
        // .level_for("hyper", log::LevelFilter::Info)
        // Output to stdout, files, and other Dispatch configurations
        .chain(std::io::stderr())
        .chain(
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .append(false)
                .open(&format!("{}.log", args.name))?,
        )
        // .chain(fern::log_file()?)
        // Apply globally
        .apply()?;

    std::panic::set_hook(Box::new(|info| {
        println!("thread panicked {info}");
    }));

    let command = args.command.as_str().parse()?;

    match command {
        Command::SignallingServer => signalling::server(args.address.as_str()).await,
        Command::Peer => Ok(peer(&args.address, &args.name).await?),
    }
}

async fn peer_inner(
    our_peer_id: Uuid,
    their_peer_id: Uuid,
    tx: mpsc::Sender<SignallingControl>,
    peer_controls: Arc<Mutex<HashMap<PeerId, mpsc::Sender<PeerControl>>>>,
    controlling: bool,
) -> Result<()> {
    let mut peer_controls = peer_controls.lock().await;

    let (control, mut event) = peer::peer(our_peer_id, their_peer_id, tx.clone(), controlling)
        .await
        .unwrap();

    peer_controls.insert(their_peer_id, control);

    let audio_player = player::audio::Player::new();
    let (audio_sink_tx, audio_sink_rx) = player::audio::sink()?;

    audio_player
        .control_tx
        .send(player::audio::PlayerControl::Volume(0.1))
        .await
        .unwrap();

    audio_player
        .control_tx
        .send(player::audio::PlayerControl::Sink(audio_sink_rx))
        .await
        .unwrap();

    let video_sink_tx = player::video::sink()?;

    tokio::spawn({
        async move {
            // NOTE(emily): Make sure to keep player alive
            let player = audio_player;
            while let Some(event) = event.recv().await {
                match event {
                    peer::PeerEvent::Audio(audio) => {
                        log::debug!("peer event audio {}", audio.len());
                        util::send("PeerEvent to Player audio", &audio_sink_tx, audio)
                            .await
                            .unwrap();
                        // audio_sink_tx.send(audio).await.unwrap();
                    }
                    peer::PeerEvent::Video(video) => {
                        log::debug!("peer event video {}", video.len());
                        util::send("PeerEvent to Player video", &video_sink_tx, video)
                            .await
                            .unwrap();
                        // video_sink_tx.send(video).await.unwrap();
                    }
                }
            }
        }
    });

    Ok(())
}

async fn peer(address: &str, name: &str) -> Result<()> {
    if name == "a" {
        console_subscriber::init();
    }

    let (tx, mut rx) = signalling::client(address).await?;

    let our_peer_id = Arc::new(Mutex::new(None));
    let last_connection_request = Arc::new(Mutex::new(None));
    let peer_controls = Arc::new(Mutex::new(
        HashMap::<PeerId, mpsc::Sender<PeerControl>>::new(),
    ));
    let connection_peer_id = Arc::new(Mutex::new(HashMap::<ConnectionId, PeerId>::new()));

    tokio::spawn({
        let tx = tx.clone();
        let our_peer_id = our_peer_id.clone();
        let last_connection_request = last_connection_request.clone();
        let peer_controls = peer_controls.clone();
        let connection_peer_id = connection_peer_id.clone();

        async move {
            while let Some(event) = rx.recv().await {
                match event {
                    signalling::SignallingEvent::Id(id) => {
                        log::info!("id {id}");
                        println!("{id}");
                        *our_peer_id.lock().await = Some(id);
                    }
                    signalling::SignallingEvent::ConectionRequest(peer_id, connection_id) => {
                        log::info!("connection request p:{peer_id} c:{connection_id}");
                        *last_connection_request.lock().await = Some(connection_id);
                        connection_peer_id
                            .lock()
                            .await
                            .insert(connection_id, peer_id);
                    }
                    signalling::SignallingEvent::Offer(peer_id, offer) => {
                        log::info!("offer p:{peer_id} {offer}");
                        let peer_controls = peer_controls.lock().await;
                        if let Some(peer_control) = peer_controls.get(&peer_id) {
                            peer_control.send(PeerControl::Offer(offer)).await.unwrap();
                        } else {
                            log::debug!("got offer for unknown peer {peer_id}");
                            log::debug!("peer_controls is {:?}", *peer_controls);
                        }
                    }
                    signalling::SignallingEvent::Answer(peer_id, answer) => {
                        log::info!("answer p:{peer_id} {answer}");
                        let peer_controls = peer_controls.lock().await;
                        if let Some(peer_control) = peer_controls.get(&peer_id) {
                            peer_control
                                .send(PeerControl::Answer(answer))
                                .await
                                .unwrap();
                        } else {
                            log::debug!("got answer for unknown peer {peer_id}");
                            log::debug!("peer_controls is {:?}", *peer_controls);
                        }
                    }
                    signalling::SignallingEvent::IceCandidate(peer_id, ice_candidate) => {
                        log::info!("ice candidate p:{peer_id} {ice_candidate}");
                        let peer_controls = peer_controls.lock().await;
                        if let Some(peer_control) = peer_controls.get(&peer_id) {
                            peer_control
                                .send(PeerControl::IceCandidate(ice_candidate))
                                .await
                                .unwrap();

                            log::info!("sent candidate to peer control");
                        } else {
                            log::debug!("got ice candidate for unknown peer {peer_id}");
                            log::debug!("peer_controls is {:?}", *peer_controls);
                        }
                    }
                    signalling::SignallingEvent::ConnectionAccepted(peer_id, connection_id) => {
                        // NOTE(emily): We sent the request so we are controlling
                        log::info!("connection accepted p:{peer_id} c:{connection_id}");

                        let our_peer_id = our_peer_id.lock().await.as_ref().unwrap().clone();

                        assert!(peer_id != our_peer_id);

                        tokio::spawn({
                            let our_peer_id = our_peer_id;
                            let tx = tx.clone();
                            let peer_controls = peer_controls.clone();

                            peer_inner(our_peer_id, peer_id, tx.clone(), peer_controls, true)
                        });
                    }
                }
            }

            log::debug!("client going down");
        }
    });

    tokio::task::spawn({
        let tx = tx.clone();
        let our_peer_id = our_peer_id.clone();
        let last_connection_request = last_connection_request.clone();
        let peer_controls = peer_controls.clone();
        async move {
            let (tx, mut rx) =
                media::produce("E:/emily/downloads/scdl/badapple1080.mp4", 144, 72).await?;

            while let Some(event) = rx.recv().await {
                match event {
                    media::MediaEvent::Audio(audio) => {
                        log::trace!("produce audio {}", audio.len());
                        let peer_controls = peer_controls.lock().await;
                        for (_, control) in peer_controls.iter() {
                            util::send(
                                "produce to peer audio",
                                &control,
                                PeerControl::Audio(audio.clone()),
                            )
                            .await
                            .unwrap();
                        }
                    }
                    media::MediaEvent::Video(video) => {
                        // log::debug!("throwing video");
                        log::trace!("produce video {}", video.len());
                        let peer_controls = peer_controls.lock().await;
                        for (_, control) in peer_controls.iter() {
                            util::send(
                                "produce to peer video",
                                &control,
                                PeerControl::Video(video.clone()),
                            )
                            .await
                            .unwrap();
                        }
                    }
                }
            }

            eyre::Ok(())
        }
    });

    tokio::task::spawn_blocking({
        let tx = tx.clone();
        let our_peer_id = our_peer_id.clone();
        let last_connection_request = last_connection_request.clone();
        let peer_controls = peer_controls.clone();
        move || {
            for line in std::io::stdin().lines() {
                if let Ok(line) = line {
                    let (command, arg) = {
                        let mut split = line.split(" ");
                        (
                            split.next().unwrap_or_default(),
                            split.next().unwrap_or_default(),
                        )
                    };

                    let tx = tx.clone();
                    let our_peer_id = our_peer_id.clone();
                    let peer_controls = peer_controls.clone();
                    let connection_peer_id = connection_peer_id.clone();

                    match command {
                        "connect" => {
                            let peer_id = Uuid::from_str(arg)?;
                            tx.blocking_send(signalling::SignallingControl::RequestConnection(
                                peer_id,
                            ))?;
                        }
                        "accept" => {
                            log::debug!("accept '{arg}'");
                            let connection_id = if arg == "" {
                                last_connection_request.try_lock().unwrap().unwrap()
                            } else {
                                Uuid::from_str(arg)?
                            };

                            tokio::spawn({
                                let tx = tx.clone();

                                async move {
                                    if let Some(peer_id) =
                                        connection_peer_id.lock().await.remove(&connection_id)
                                    {
                                        let our_peer_id =
                                            our_peer_id.lock().await.as_ref().unwrap().clone();

                                        assert!(peer_id != our_peer_id);

                                        peer_inner(our_peer_id, peer_id, tx, peer_controls, false)
                                            .await
                                            .unwrap();
                                    } else {
                                        log::debug!("Unknown connection id {connection_id}");
                                    }
                                }
                            });

                            tx.blocking_send(signalling::SignallingControl::AcceptConnection(
                                connection_id,
                            ))?;
                        }
                        command => log::info!("Unknown command {command}"),
                    }
                }
            }

            eyre::Ok(())
        }
    })
    .await??;

    Ok(())
}
