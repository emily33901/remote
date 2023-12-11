use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use webrtc::{data_channel::RTCDataChannel, peer_connection::RTCPeerConnection};

use crate::{
    channel::{channel, ChannelControl, ChannelEvent, ChannelStorage},
    chunk::{chunk, Chunk, ChunkControl},
    util,
};

use eyre::{eyre, Result};

pub(crate) enum VideoEvent {
    Video(Vec<u8>),
}

pub(crate) enum VideoControl {
    Video(Vec<u8>),
}

pub(crate) async fn video_channel(
    channel_storage: ChannelStorage,
    peer_connection: Arc<RTCPeerConnection>,
    controlling: bool,
) -> Result<(mpsc::Sender<VideoControl>, mpsc::Receiver<VideoEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(10);
    let (event_tx, event_rx) = mpsc::channel(10);

    let (tx, mut rx) = channel(
        channel_storage,
        peer_connection,
        "video",
        controlling,
        Some(
            webrtc::data_channel::data_channel_init::RTCDataChannelInit {
                ordered: Some(false),
                max_packet_life_time: Some(0),
                max_retransmits: Some(0),
                ..Default::default()
            },
        ),
    )
    .await?;

    let (chunk_tx, mut chunk_rx) = chunk::<Vec<u8>>(10_000).await?;

    tokio::spawn({
        let tx = tx.clone();
        let chunk_tx = chunk_tx.clone();
        async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ChannelEvent::Open(channel) => {}
                    ChannelEvent::Close(channel) => {}
                    ChannelEvent::Message(channel, message) => {
                        let chunk: Chunk = bincode::deserialize(&message.data).unwrap();
                        util::send(
                            "video channel event to chunk control",
                            &chunk_tx,
                            ChunkControl::Chunk(chunk),
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
        let tx = tx.clone();
        let event_tx = event_tx.clone();
        async move {
            while let Some(control) = chunk_rx.recv().await {
                match control {
                    crate::chunk::ChunkEvent::Whole(whole) => {
                        log::debug!("video reassembled whole frame");
                        util::send(
                            "chunk event whole to video event",
                            &event_tx,
                            VideoEvent::Video(whole),
                        )
                        .await
                        .unwrap();
                    }
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
