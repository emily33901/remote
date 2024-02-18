mod audio;
mod chunk;
mod logic;
mod peer;
mod player;
mod video;

use std::str::FromStr;
use std::sync::Arc;
use std::{collections::HashMap, fmt::Display};

use clap::Parser;
use eyre::Result;
use peer::PeerControl;
use rtc;
use signal::SignallingControl;
use signal::{ConnectionId, PeerId};

use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

const ARBITRARY_CHANNEL_LIMIT: usize = 10;

#[derive(Debug, Clone)]
enum Command {
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

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
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
    #[arg(short, long, action)]
    produce: bool,

    command: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Configure logger at runtime
    let args = Args::parse();

    dotenv::dotenv()?;

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
        .level(log::LevelFilter::from_str(&std::env::var("log_level")?)?)
        // .level_for("remote", log::LevelFilter::Debug)
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
        // .chain(
        //     std::fs::OpenOptions::new()
        //         .write(true)
        //         .create(true)
        //         .append(false)
        //         .open(&format!("{}.log", args.name))?,
        // )
        // .chain(fern::log_file()?)
        // Apply globally
        .apply()?;

    log::info!("remote - '{}' '{}'", args.command, args.address);

    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::capture();
        eprintln!("thread panicked {info}");
        eprintln!("backtrace\n{backtrace}");
    }));

    let command = args.command.as_str().parse()?;

    match command {
        Command::Peer => Ok(peer(&args.address, &args.produce).await?),
    }
}

async fn peer_connected(
    our_peer_id: PeerId,
    their_peer_id: PeerId,
    tx: mpsc::Sender<SignallingControl>,
    peer_controls: Arc<Mutex<HashMap<PeerId, mpsc::Sender<PeerControl>>>>,
    controlling: bool,
) -> Result<()> {
    let width = u32::from_str(&std::env::var("width")?)?;
    let height = u32::from_str(&std::env::var("height")?)?;
    let bitrate = u32::from_str(&std::env::var("bitrate")?)?;
    let framerate = u32::from_str(&std::env::var("framerate")?)?;

    let api = rtc::Api::from_str(&std::env::var("webrtc_api")?)?;

    let mut peer_controls = peer_controls.lock().await;

    let (control, mut event) = peer::peer(
        api,
        our_peer_id.clone(),
        their_peer_id.clone(),
        tx.clone(),
        controlling,
    )
    .await?;

    peer_controls.insert(their_peer_id, control);

    let audio_player = player::audio::Player::new();
    let (audio_sink_tx, audio_sink_rx) = player::audio::sink()?;

    audio_player
        .control_tx
        .send(player::audio::PlayerControl::Volume(0.1))
        .await?;

    audio_player
        .control_tx
        .send(player::audio::PlayerControl::Sink(audio_sink_rx))
        .await?;

    let (h264_control, mut h264_event) = media::decoder::Decoder::OpenH264
        .run(width, height, framerate, bitrate)
        .await?;

    let video_sink_tx = player::video::sink(width, height, "player-window")?;

    tokio::spawn({
        async move {
            while let Some(event) = h264_event.recv().await {
                match event {
                    media::decoder::DecoderEvent::Frame(tex, time) => {
                        video_sink_tx.send((tex, time)).await.unwrap()
                    }
                }
            }
        }
    });

    tokio::spawn({
        let our_peer_id = our_peer_id.clone();

        async move {
            // NOTE(emily): Make sure to keep player alive
            let _player = audio_player;

            // let file_sink = media::file_sink::file_sink(
            //     std::path::Path::new(&format!("test-{our_peer_id}.mp4")),
            //     width,
            //     height,
            //     framerate,
            //     bitrate,
            // )
            // .unwrap();

            // let mut i = 0;

            while let Some(event) = event.recv().await {
                match event {
                    peer::PeerEvent::Audio(audio) => {
                        // log::debug!("peer event audio {}", audio.len());
                        audio_sink_tx.send(audio).await.unwrap();
                        // audio_sink_tx.send(audio).await.unwrap();
                    }
                    peer::PeerEvent::Video(video) => {
                        // log::debug!("peer event video {}", video.data.len());
                        h264_control
                            .send(media::decoder::DecoderControl::Data(video.clone()))
                            .await
                            .unwrap();

                        // match i {
                        //     0..=1000 => file_sink
                        //         .send(media::file_sink::FileSinkControl::Video(video))
                        //         .await
                        //         .unwrap(),
                        //     1001 => file_sink
                        //         .send(media::file_sink::FileSinkControl::Done)
                        //         .await
                        //         .unwrap(),
                        //     _ => {
                        //         log::info!("!! DONE")
                        //     }
                        // }
                        // i += 1;
                    }
                    peer::PeerEvent::Error(error) => {
                        log::warn!("peer event error {error:?}");
                        break;
                    }
                }
            }
        }
    });

    Ok(())
}

async fn peer(address: &str, produce: &bool) -> Result<()> {
    telemetry::client::sink().await;

    let width = u32::from_str(&std::env::var("width")?)?;
    let height = u32::from_str(&std::env::var("height")?)?;
    let bitrate = u32::from_str(&std::env::var("bitrate")?)?;
    let framerate = u32::from_str(&std::env::var("framerate")?)?;

    let (signal_tx, mut signal_rx) = signal::client(address).await?;

    let our_peer_id = Arc::new(Mutex::new(None));
    let last_connection_request = Arc::new(Mutex::new(None));
    let peer_controls = Arc::new(Mutex::new(
        HashMap::<PeerId, mpsc::Sender<PeerControl>>::new(),
    ));
    let connection_peer_id = Arc::new(Mutex::new(HashMap::<ConnectionId, PeerId>::new()));

    tokio::spawn({
        let tx = signal_tx.clone();
        let our_peer_id = our_peer_id.clone();
        let last_connection_request = last_connection_request.clone();
        let peer_controls = peer_controls.clone();
        let connection_peer_id = connection_peer_id.clone();

        async move {
            while let Some(event) = signal_rx.recv().await {
                match event {
                    signal::SignallingEvent::Id(id) => {
                        log::info!("id {id}");
                        println!("{id}");
                        *our_peer_id.lock().await = Some(id);
                    }
                    signal::SignallingEvent::ConectionRequest(peer_id, connection_id) => {
                        log::info!("connection request p:{peer_id} c:{connection_id}");
                        *last_connection_request.lock().await = Some(connection_id);
                        connection_peer_id
                            .lock()
                            .await
                            .insert(connection_id, peer_id);
                    }
                    signal::SignallingEvent::Offer(peer_id, offer) => {
                        log::info!("offer p:{peer_id} {offer}");
                        let peer_controls = peer_controls.lock().await;
                        if let Some(peer_control) = peer_controls.get(&peer_id) {
                            peer_control.send(PeerControl::Offer(offer)).await.unwrap();
                        } else {
                            log::debug!("got offer for unknown peer {peer_id}");
                            log::debug!("peer_controls is {:?}", *peer_controls);
                        }
                    }
                    signal::SignallingEvent::Answer(peer_id, answer) => {
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
                    signal::SignallingEvent::IceCandidate(peer_id, ice_candidate) => {
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
                    signal::SignallingEvent::ConnectionAccepted(peer_id, connection_id) => {
                        // NOTE(emily): We sent the request so we are controlling
                        log::info!("connection accepted p:{peer_id} c:{connection_id}");

                        let our_peer_id = our_peer_id.lock().await.as_ref().unwrap().clone();

                        assert!(peer_id != our_peer_id);

                        tokio::spawn({
                            let our_peer_id = our_peer_id;
                            let tx = tx.clone();
                            let peer_controls = peer_controls.clone();

                            peer_connected(our_peer_id, peer_id, tx.clone(), peer_controls, true)
                        });
                    }
                    signal::SignallingEvent::Error(error) => {
                        log::info!("signalling error {error:?}");
                    }
                }
            }

            log::info!("client going down");
        }
    });

    if *produce {
        tokio::task::spawn({
            let _tx = signal_tx.clone();
            let _our_peer_id = our_peer_id.clone();
            let _last_connection_request = last_connection_request.clone();
            let peer_controls = peer_controls.clone();
            async move {
                match async move {
                    let maybe_file = std::env::var("media_filename").ok();

                    let (_tx, mut rx) = if let Some(file) = maybe_file {
                        media::produce::produce(&file, width, height, framerate, bitrate).await?
                    } else {
                        media::desktop_duplication::duplicate_desktop(
                            width, height, framerate, bitrate,
                        )
                        .await?
                    };

                    while let Some(event) = rx.recv().await {
                        match event {
                            media::produce::MediaEvent::Audio(audio) => {
                                log::trace!("produce audio {}", audio.len());
                                let peer_controls = peer_controls.lock().await;
                                for (_, control) in peer_controls.iter() {
                                    control.send(PeerControl::Audio(audio.clone())).await?;
                                }
                            }
                            media::produce::MediaEvent::Video(video) => {
                                // log::debug!("throwing video");
                                log::trace!("produce video {}", video.data.len());
                                let peer_controls = peer_controls.lock().await;
                                for (_, control) in peer_controls.iter() {
                                    control.send(PeerControl::Video(video.clone())).await?;
                                }
                            }
                        }
                    }

                    eyre::Ok(())
                }
                .await
                {
                    Ok(_) => {
                        log::info!("produce down ok")
                    }
                    Err(err) => {
                        log::error!("produce down err {err}")
                    }
                }

                eyre::Ok(())
            }
        });
    }

    tokio::task::spawn_blocking({
        let tx = signal_tx.clone();
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
                            // let peer_id = Uuid::from_str(arg)?;
                            let peer_id = arg.into();
                            tx.blocking_send(signal::SignallingControl::RequestConnection(
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

                                        peer_connected(
                                            our_peer_id,
                                            peer_id,
                                            tx,
                                            peer_controls,
                                            false,
                                        )
                                        .await
                                        .unwrap();
                                    } else {
                                        log::debug!("Unknown connection id {connection_id}");
                                    }
                                }
                            });

                            tx.blocking_send(signal::SignallingControl::AcceptConnection(
                                connection_id,
                            ))?;
                        }
                        "die" => {
                            tokio::spawn(async move {
                                for (_, control) in peer_controls.lock().await.drain() {
                                    control.send(PeerControl::Die).await.unwrap();
                                }
                            });
                        }
                        "quit" | "exit" | "q" => {
                            std::process::exit(0);
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
