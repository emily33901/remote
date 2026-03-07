use anyhow::Result;
use media::VideoBuffer;
use rtc::{ChannelControl, ChannelEvent, PeerConnection};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::Instrument;

use crate::protocol::LogicMessage;
use crate::ARBITRARY_CHANNEL_LIMIT;

#[derive(Debug, Serialize, Deserialize)]
pub enum AudioMessage {
    Audio(Vec<u8>),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum VideoMessage {
    Video(VideoBuffer),
}

pub enum AudioControl {
    Audio(Vec<u8>),
}

pub enum AudioEvent {
    Audio(Vec<u8>),
}

pub enum VideoControl {
    Video(VideoBuffer),
}

pub enum VideoEvent {
    Video(VideoBuffer),
}

pub async fn logic_channel(
    peer_connection: &dyn PeerConnection,
    controlling: bool,
) -> Result<(mpsc::Sender<LogicMessage>, mpsc::Receiver<LogicMessage>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (tx, mut rx) = peer_connection.channel("logic", controlling, None).await?;

    tokio::spawn({
        let weak_control_tx = control_tx.downgrade();
        async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ChannelEvent::Open => {}
                    ChannelEvent::Close => {}
                    ChannelEvent::Message(data) => {
                        let Ok(message) = bincode::deserialize(&data) else {
                            continue;
                        };

                        match message {
                            LogicMessage::Ping => {
                                if let Some(tx) = weak_control_tx.upgrade() {
                                    let _ = tx.send(LogicMessage::Pong).await;
                                }
                            }
                            LogicMessage::Pong => {}
                            message => {
                                let _ = event_tx.send(message).await;
                            }
                        }
                    }
                }
            }

            anyhow::Ok(())
        }
        .in_current_span()
    });

    tokio::spawn({
        let tx = tx.clone();
        async move {
            while let Some(control) = control_rx.recv().await {
                let Ok(encoded) = bincode::serialize(&control) else {
                    continue;
                };
                let _ = tx.send(ChannelControl::Send(encoded)).await;
            }

            anyhow::Ok(())
        }
        .in_current_span()
    });

    tokio::spawn({
        let weak_control_tx = control_tx.downgrade();
        async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                ticker.tick().await;
                if let Some(tx) = weak_control_tx.upgrade() {
                    let _ = tx.send(LogicMessage::Ping).await;
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}

pub async fn audio_channel(
    peer_connection: &dyn PeerConnection,
    controlling: bool,
) -> Result<(mpsc::Sender<AudioControl>, mpsc::Receiver<AudioEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (tx, mut rx) = peer_connection.channel("audio", controlling, None).await?;

    tokio::spawn({
        async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ChannelEvent::Open => {}
                    ChannelEvent::Close => {}
                    ChannelEvent::Message(data) => {
                        let Ok(message) = bincode::deserialize::<AudioMessage>(&data) else {
                            continue;
                        };
                        match message {
                            AudioMessage::Audio(audio) => {
                                let _ = event_tx.send(AudioEvent::Audio(audio)).await;
                            }
                        }
                    }
                }
            }

            anyhow::Ok(())
        }
        .in_current_span()
    });

    tokio::spawn({
        let tx = tx.clone();
        async move {
            while let Some(control) = control_rx.recv().await {
                match control {
                    AudioControl::Audio(audio) => {
                        let message = AudioMessage::Audio(audio);
                        let Ok(encoded) = bincode::serialize(&message) else {
                            continue;
                        };
                        let _ = tx.send(ChannelControl::Send(encoded)).await;
                    }
                }
            }

            anyhow::Ok(())
        }
        .in_current_span()
    });

    Ok((control_tx, event_rx))
}

pub async fn video_channel(
    peer_connection: &dyn PeerConnection,
    controlling: bool,
) -> Result<(mpsc::Sender<VideoControl>, mpsc::Receiver<VideoEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (tx, mut rx) = peer_connection.channel("video", controlling, None).await?;

    tokio::spawn({
        async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ChannelEvent::Open => {}
                    ChannelEvent::Close => {}
                    ChannelEvent::Message(data) => {
                        let Ok(message) = bincode::deserialize::<VideoMessage>(&data) else {
                            continue;
                        };
                        match message {
                            VideoMessage::Video(video) => {
                                let _ = event_tx.send(VideoEvent::Video(video)).await;
                            }
                        }
                    }
                }
            }

            anyhow::Ok(())
        }
        .in_current_span()
    });

    tokio::spawn({
        let tx = tx.clone();
        async move {
            while let Some(control) = control_rx.recv().await {
                match control {
                    VideoControl::Video(video) => {
                        let message = VideoMessage::Video(video);
                        let Ok(encoded) = bincode::serialize(&message) else {
                            continue;
                        };
                        let _ = tx.send(ChannelControl::Send(encoded)).await;
                    }
                }
            }

            anyhow::Ok(())
        }
        .in_current_span()
    });

    Ok((control_tx, event_rx))
}
