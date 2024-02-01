use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::{
    chunk::{assembly, chunk, AssemblyControl, Chunk},
    media::encoder::FrameIsKeyframe,
    rtc::{ChannelControl, ChannelEvent, ChannelOptions, PeerConnection},
    ARBITRARY_CHANNEL_LIMIT,
};

use eyre::Result;
use std::str::FromStr;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct VideoBuffer {
    pub(crate) data: Vec<u8>,
    pub(crate) sequence_header: Option<Vec<u8>>,
    pub(crate) time: std::time::SystemTime,
    pub(crate) duration: std::time::Duration,
    pub(crate) key_frame: FrameIsKeyframe,
}

pub(crate) enum VideoEvent {
    Video(VideoBuffer),
}

pub(crate) enum VideoControl {
    Video(VideoBuffer),
}

pub(crate) async fn video_channel(
    peer_connection: Arc<dyn PeerConnection>,
    controlling: bool,
) -> Result<(mpsc::Sender<VideoControl>, mpsc::Receiver<VideoEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "video-control").await;
    telemetry::client::watch_channel(&event_tx, "video-event").await;

    let (tx, mut rx) = peer_connection
        .channel(
            "video",
            controlling,
            Some(ChannelOptions {
                ordered: Some(false),
                max_retransmits: Some(0),
            }),
        )
        .await?;

    let video_chunk_size = usize::from_str(&std::env::var("video_chunk_size")?)?;

    let (chunk_tx, mut chunk_rx) = chunk::<VideoBuffer>(video_chunk_size).await?;
    let (assembly_tx, mut assembly_rx) = assembly::<VideoBuffer>().await?;

    tokio::spawn({
        let _tx = tx.clone();
        let assembly_tx = assembly_tx.clone();
        async move {
            match tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    match event {
                        ChannelEvent::Open => {}
                        ChannelEvent::Close => {}
                        ChannelEvent::Message(data) => {
                            let chunk: Chunk = bincode::deserialize(&data).unwrap();
                            assembly_tx.send(AssemblyControl::Chunk(chunk)).await?
                        }
                    }
                }

                eyre::Ok(())
            })
            .await
            {
                Ok(r) => match r {
                    Ok(_) => {}
                    Err(err) => {
                        log::error!("video channel event error {err}");
                    }
                },
                Err(err) => {
                    log::error!("video channel event join error {err}");
                }
            }
        }
    });

    tokio::spawn({
        let chunk_tx = chunk_tx.clone();
        async move {
            match tokio::spawn(async move {
                while let Some(control) = control_rx.recv().await {
                    match control {
                        VideoControl::Video(video) => {
                            let deadline = video.time;
                            // if let Ok(t) = deadline.elapsed() {
                            //     log::warn!(
                            //         "throwing expired frame {}ms in the past",
                            //         t.as_millis()
                            //     );
                            // } else
                            {
                                chunk_tx
                                    .send(crate::chunk::ChunkControl::Whole(video, deadline))
                                    .await?;
                            }
                        }
                    }
                }
                eyre::Ok(())
            })
            .await
            {
                Ok(r) => match r {
                    Ok(_) => {}
                    Err(err) => {
                        log::error!("video channel control error {err}");
                    }
                },
                Err(err) => {
                    log::error!("video channel control join error {err}");
                }
            }
        }
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            match tokio::spawn(async move {
                while let Some(control) = assembly_rx.recv().await {
                    match control {
                        crate::chunk::AssemblyEvent::Whole(whole) => {
                            event_tx.send(VideoEvent::Video(whole)).await?;
                        }
                    }
                }
                eyre::Ok(())
            })
            .await
            {
                Ok(r) => match r {
                    Ok(_) => {}
                    Err(err) => {
                        log::error!("video channel assembly event error {err}");
                    }
                },
                Err(err) => {
                    log::error!("video channel assembly event join error {err}");
                }
            }
        }
    });

    tokio::spawn({
        let tx = tx.clone();
        async move {
            match tokio::spawn(async move {
                while let Some(control) = chunk_rx.recv().await {
                    match control {
                        crate::chunk::ChunkEvent::Chunk(chunk) => {
                            tx.send(ChannelControl::Send(bincode::serialize(&chunk)?))
                                .await?;
                        }
                    }
                }

                eyre::Ok(())
            })
            .await
            {
                Ok(r) => match r {
                    Ok(_) => {}
                    Err(err) => {
                        log::error!("video channel chunk event error {err}");
                    }
                },
                Err(err) => {
                    log::error!("video channel chunk event join error {err}");
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}
