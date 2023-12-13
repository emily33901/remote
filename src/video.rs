use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use webrtc::peer_connection::RTCPeerConnection;

use crate::{
    channel::{channel, ChannelControl, ChannelEvent, ChannelStorage},
    chunk::{assembly, chunk, AssemblyControl, Chunk},
    util, ARBITRARY_CHANNEL_LIMIT,
};

use eyre::Result;
use std::str::FromStr;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct VideoBuffer {
    pub(crate) data: Vec<u8>,
    pub(crate) time: std::time::SystemTime,
}

pub(crate) enum VideoEvent {
    Video(VideoBuffer),
}

pub(crate) enum VideoControl {
    Video(VideoBuffer),
}

pub(crate) async fn video_channel(
    channel_storage: ChannelStorage,
    peer_connection: Arc<RTCPeerConnection>,
    controlling: bool,
) -> Result<(mpsc::Sender<VideoControl>, mpsc::Receiver<VideoEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, &format!("video-control")).await;
    telemetry::client::watch_channel(&event_tx, &format!("video-event")).await;

    let (tx, mut rx) = channel(
        channel_storage,
        peer_connection,
        "video",
        controlling,
        Some(
            webrtc::data_channel::data_channel_init::RTCDataChannelInit {
                ordered: Some(false),
                max_retransmits: Some(0),
                ..Default::default()
            },
        ),
    )
    .await?;

    let video_ttl = u64::from_str(&std::env::var("video_ttl")?)?;

    let (chunk_tx, mut chunk_rx) =
        chunk::<VideoBuffer>(20_000, std::time::Duration::from_millis(video_ttl)).await?;
    let (assembly_tx, mut assembly_rx) = assembly::<VideoBuffer>().await?;

    tokio::spawn({
        let _tx = tx.clone();
        let assembly_tx = assembly_tx.clone();
        async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ChannelEvent::Open(_channel) => {}
                    ChannelEvent::Close(_channel) => {}
                    ChannelEvent::Message(_channel, message) => {
                        let chunk: Chunk = bincode::deserialize(&message.data).unwrap();
                        util::send(
                            "video channel event to assembly control",
                            &assembly_tx,
                            AssemblyControl::Chunk(chunk),
                        )
                        .await
                        .unwrap();
                    }
                }
            }
        }
    });

    tokio::spawn({
        let chunk_tx = chunk_tx.clone();
        async move {
            while let Some(control) = control_rx.recv().await {
                match control {
                    VideoControl::Video(video) => {
                        util::send(
                            "video control to chunk control",
                            &chunk_tx,
                            crate::chunk::ChunkControl::Whole(video),
                        )
                        .await
                        .unwrap();
                    }
                }
            }
        }
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            while let Some(control) = assembly_rx.recv().await {
                match control {
                    crate::chunk::AssemblyEvent::Whole(whole) => {
                        util::send("assembly video event", &event_tx, VideoEvent::Video(whole))
                            .await
                            .unwrap();
                    }
                }
            }
        }
    });

    tokio::spawn({
        let tx = tx.clone();
        async move {
            while let Some(control) = chunk_rx.recv().await {
                match control {
                    crate::chunk::ChunkEvent::Chunk(chunk) => {
                        util::send(
                            "chunk event chunk to channel control",
                            &tx,
                            ChannelControl::Send(bincode::serialize(&chunk).unwrap()),
                        )
                        .await
                        .unwrap();
                        // log::debug!("video chunking frame");
                    }
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}
