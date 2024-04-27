mod audio;
mod chunk;
mod config;
mod logic;
mod peer;
mod player;
mod ui;
mod video;

use crate::config::Config;
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
use tracing::level_filters::LevelFilter;
use uuid::Uuid;


const ARBITRARY_CHANNEL_LIMIT: usize = 10;

#[derive(Debug, Clone)]
enum Command {
    Peer,
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
            "peer" => Ok(Self::Peer),
            "ui" => Ok(Self::Ui),
            _ => Err(CommandParseError),
        }
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, action)]
    produce: bool,

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
        .add_directive("webrtc_sctp::association=info".parse()?)
        .add_directive("webrtc_sctp::association::association_internal=info".parse()?)
        .add_directive("webrtc_sctp::stream=info".parse()?);

    tracing_subscriber::fmt::fmt()
        .with_env_filter(filter)
        // .with_max_level(LevelFilter::DEBUG)
        .pretty()
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
        Command::Peer => Ok(peer(&args.produce).await?),
    }
}

async fn peer_connected(
    our_peer_id: PeerId,
    their_peer_id: PeerId,
    tx: mpsc::Sender<SignallingControl>,
    peer_controls: Arc<Mutex<HashMap<PeerId, mpsc::Sender<PeerControl>>>>,
    controlling: bool,
) -> Result<()> {
    let config = Config::load();

    let mut peer_controls = peer_controls.lock().await;

    let (control, mut event) = peer::peer(
        config.webrtc_api,
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

    let (h264_control, mut h264_event) = config
        .decoder_api
        .run(
            config.width,
            config.height,
            config.framerate,
            config.bitrate,
        )
        .await?;

    let video_sink_tx = player::video::sink(config.width, config.height, "player-window")?;

    tokio::spawn({
        async move {
            while let Some(event) = h264_event.recv().await {
                match event {
                    media::decoder::DecoderEvent::Frame(tex, time) => {
                        // Try and give video to player otherwise drop it.
                        // TODO(emily): Probably back pressure
                        let _ = video_sink_tx.send((tex, time)).await;
                    }
                }
            }
        }
    });

    tokio::spawn({
        let _our_peer_id = our_peer_id.clone();

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
                        // tracing::debug!("peer event audio {}", audio.len());
                        audio_sink_tx.send(audio).await.unwrap();
                        // audio_sink_tx.send(audio).await.unwrap();
                    }
                    peer::PeerEvent::Video(video) => {
                        // tracing::debug!("peer event video {}", video.data.len());
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
                        //         tracing::info!("!! DONE")
                        //     }
                        // }
                        // i += 1;
                    }
                    peer::PeerEvent::Error(error) => {
                        tracing::warn!("peer event error {error:?}");
                        break;
                    }
                    peer::PeerEvent::StreamRequest(request) => {
                        tracing::info!(?request, "stream request")
                    }
                    peer::PeerEvent::RequestStreamResponse(response) => {
                        tracing::info!(?response, "stream response")
                    }
                }
            }
        }
    });

    Ok(())
}

async fn peer(produce: &bool) -> Result<()> {
    let config = Config::load();

    telemetry::client::sink().await;

    let (signal_tx, mut signal_rx) = signal::client(&config.signal_server).await?;

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
                        tracing::info!("id {id}");
                        println!("{id}");
                        *our_peer_id.lock().await = Some(id);
                    }
                    signal::SignallingEvent::ConectionRequest(peer_id, connection_id) => {
                        tracing::info!("connection request p:{peer_id} c:{connection_id}");
                        *last_connection_request.lock().await = Some(connection_id);
                        connection_peer_id
                            .lock()
                            .await
                            .insert(connection_id, peer_id);
                    }
                    signal::SignallingEvent::Offer(peer_id, offer) => {
                        tracing::info!("offer p:{peer_id} {offer}");
                        let peer_controls = peer_controls.lock().await;
                        if let Some(peer_control) = peer_controls.get(&peer_id) {
                            peer_control.send(PeerControl::Offer(offer)).await.unwrap();
                        } else {
                            tracing::debug!("got offer for unknown peer {peer_id}");
                            tracing::debug!("peer_controls is {:?}", *peer_controls);
                        }
                    }
                    signal::SignallingEvent::Answer(peer_id, answer) => {
                        tracing::info!("answer p:{peer_id} {answer}");
                        let peer_controls = peer_controls.lock().await;
                        if let Some(peer_control) = peer_controls.get(&peer_id) {
                            peer_control
                                .send(PeerControl::Answer(answer))
                                .await
                                .unwrap();
                        } else {
                            tracing::debug!("got answer for unknown peer {peer_id}");
                            tracing::debug!("peer_controls is {:?}", *peer_controls);
                        }
                    }
                    signal::SignallingEvent::IceCandidate(peer_id, ice_candidate) => {
                        tracing::info!("ice candidate p:{peer_id} {ice_candidate}");
                        let peer_controls = peer_controls.lock().await;
                        if let Some(peer_control) = peer_controls.get(&peer_id) {
                            peer_control
                                .send(PeerControl::IceCandidate(ice_candidate))
                                .await
                                .unwrap();

                            tracing::info!("sent candidate to peer control");
                        } else {
                            tracing::debug!("got ice candidate for unknown peer {peer_id}");
                            tracing::debug!("peer_controls is {:?}", *peer_controls);
                        }
                    }
                    signal::SignallingEvent::ConnectionAccepted(peer_id, connection_id) => {
                        // NOTE(emily): We sent the request so we are controlling
                        tracing::info!("connection accepted p:{peer_id} c:{connection_id}");

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
                        tracing::info!("signalling error {error:?}");
                    }
                }
            }

            tracing::info!("client going down");
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
                    // let maybe_file = std::env::var("media_filename").ok();

                    let (_tx, mut rx) = if let Some(file) = config.media_filename.as_ref() {
                        media::produce::produce(
                            config.encoder_api,
                            file,
                            config.width,
                            config.height,
                            config.framerate,
                            config.bitrate,
                        )
                        .await?
                    } else {
                        media::desktop_duplication::duplicate_desktop(
                            config.encoder_api,
                            config.width,
                            config.height,
                            config.framerate,
                            config.bitrate,
                        )
                        .await?
                    };

                    while let Some(event) = rx.recv().await {
                        match event {
                            media::produce::MediaEvent::Audio(audio) => {
                                tracing::trace!("produce audio {}", audio.len());
                                let peer_controls = peer_controls.lock().await;
                                for (_, control) in peer_controls.iter() {
                                    control.send(PeerControl::Audio(audio.clone())).await?;
                                }
                            }
                            media::produce::MediaEvent::Video(video) => {
                                // tracing::debug!("throwing video");
                                tracing::trace!("produce video {}", video.data.len());
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
                        tracing::info!("produce down ok")
                    }
                    Err(err) => {
                        tracing::error!("produce down err {err}")
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
                            tracing::debug!("accept '{arg}'");
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
                                        tracing::debug!("Unknown connection id {connection_id}");
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

                        command => tracing::info!("Unknown command {command}"),
                    }
                }
            }

            tracing::warn!("stdin is done");

            eyre::Ok(())
        }
    })
    .await??;

    Ok(())
}
