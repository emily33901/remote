use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use media::VideoBuffer;
use rtc::{ChannelControl, ChannelEvent, ChannelOptions, PeerConnection};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::Instrument;

use crate::protocol::LogicMessage;
use crate::ARBITRARY_CHANNEL_LIMIT;

const DEFAULT_VIDEO_CHUNK_SIZE: usize = 16 * 1024;
const VIDEO_FRAME_TTL: Duration = Duration::from_millis(100);
const ASSEMBLY_CLEANUP_INTERVAL: Duration = Duration::from_secs(1);

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

#[derive(Serialize, Deserialize)]
struct Chunk {
    data: Vec<u8>,
    id: u32,
    part: u32,
    total: u32,
    deadline: SystemTime,
}

impl PartialEq for Chunk {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.part == other.part && self.total == other.total
    }
}

impl Eq for Chunk {}

impl Hash for Chunk {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.part.hash(state);
        self.total.hash(state);
    }
}

enum ChunkControl<T> {
    Whole(T, SystemTime),
}

enum ChunkEvent {
    Chunk(Chunk),
}

enum AssemblyControl {
    Chunk(Chunk),
}

enum AssemblyEvent<T> {
    Whole(T),
}

async fn chunk<T: Serialize + Send + 'static>(
    chunk_size: usize,
) -> Result<(mpsc::Sender<ChunkControl<T>>, mpsc::Receiver<ChunkEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    tokio::spawn({
        async move {
            let mut next_chunk_id: u32 = 0;

            while let Some(control) = control_rx.recv().await {
                match control {
                    ChunkControl::Whole(v, deadline) => {
                        let Ok(encoded) = bincode::serialize(&v) else {
                            continue;
                        };

                        let total = ((encoded.len()) / chunk_size)
                            + if encoded.len() % chunk_size == 0 { 0 } else { 1 };

                        if total == 0 {
                            continue;
                        }

                        let chunk_id = next_chunk_id;
                        next_chunk_id = next_chunk_id.wrapping_add(1);

                        for (i, chunk_data) in encoded.chunks(chunk_size).enumerate() {
                            if deadline.elapsed().is_ok() {
                                tracing::warn!("chunk {chunk_id} expired during chunking");
                                break;
                            }

                            let chunk = Chunk {
                                data: chunk_data.to_vec(),
                                id: chunk_id,
                                part: i as u32,
                                total: total as u32,
                                deadline,
                            };

                            if event_tx.send(ChunkEvent::Chunk(chunk)).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }

            anyhow::Ok(())
        }
        .in_current_span()
    });

    Ok((control_tx, event_rx))
}

async fn assembly<T: for<'de> Deserialize<'de> + Send + 'static>(
) -> Result<(mpsc::Sender<AssemblyControl>, mpsc::Receiver<AssemblyEvent<T>>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    tokio::spawn({
        async move {
            let mut chunk_arrangement: HashMap<u32, HashSet<Chunk>> = HashMap::new();
            let mut ticker = tokio::time::interval(ASSEMBLY_CLEANUP_INTERVAL);

            fn remove_elapsed_chunks(chunk_arrangement: &mut HashMap<u32, HashSet<Chunk>>) {
                let expired_ids: Vec<u32> = chunk_arrangement
                    .iter()
                    .filter_map(|(id, chunks)| {
                        chunks.iter().any(|c| c.deadline.elapsed().is_ok()).then_some(*id)
                    })
                    .collect();

                for id in expired_ids {
                    if let Some(chunks) = chunk_arrangement.remove(&id) {
                        tracing::trace!(
                            chunks_removed = chunks.len(),
                            "removing expired incomplete chunks"
                        );
                    }
                }
            }

            loop {
                tokio::select! {
                    control = control_rx.recv() => {
                        match control {
                            Some(AssemblyControl::Chunk(chunk)) => {
                                if chunk.deadline.elapsed().is_ok() {
                                    tracing::trace!("ignoring expired chunk");
                                    continue;
                                }

                                let total = chunk.total as usize;
                                let chunk_id = chunk.id;

                                if total == 1 {
                                    let Ok(v) = bincode::deserialize(&chunk.data) else {
                                        tracing::warn!("failed to deserialize single-chunk message");
                                        continue;
                                    };
                                    let _ = event_tx.send(AssemblyEvent::Whole(v)).await;
                                    continue;
                                }

                                let was_new = chunk_arrangement
                                    .entry(chunk_id)
                                    .or_default()
                                    .insert(chunk);

                                if !was_new {
                                    continue;
                                }

                                let chunks = chunk_arrangement.get(&chunk_id).unwrap();
                                if chunks.len() == total {
                                    let mut chunks: Vec<_> = chunk_arrangement.remove(&chunk_id).unwrap().into_iter().collect();
                                    chunks.sort_by_key(|c| c.part);

                                    let mut data = Vec::with_capacity(
                                        chunks.iter().map(|c| c.data.len()).sum()
                                    );
                                    for chunk in chunks {
                                        data.extend(chunk.data);
                                    }

                                    match bincode::deserialize::<T>(&data) {
                                        Ok(v) => {
                                            let _ = event_tx.send(AssemblyEvent::Whole(v)).await;
                                        }
                                        Err(e) => {
                                            tracing::warn!("failed to deserialize reassembled message: {e}");
                                        }
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                    _ = ticker.tick() => {
                        remove_elapsed_chunks(&mut chunk_arrangement);
                    }
                }
            }

            anyhow::Ok(())
        }
        .in_current_span()
    });

    Ok((control_tx, event_rx))
}

pub async fn logic_channel(
    peer_connection: &dyn PeerConnection,
    controlling: bool,
) -> Result<(mpsc::Sender<LogicMessage>, mpsc::Receiver<LogicMessage>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (tx, mut rx) = peer_connection.channel("logic", controlling, None).await?;
    tracing::info!("logic_channel: created (controlling={})", controlling);

    let is_open = Arc::new(AtomicBool::new(false));
    let open_notify = Arc::new(tokio::sync::Notify::new());

    tokio::spawn({
        let weak_control_tx = control_tx.downgrade();
        let is_open = is_open.clone();
        let open_notify = open_notify.clone();
        async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ChannelEvent::Open => {
                        tracing::info!("logic_channel: channel opened");
                        is_open.store(true, Ordering::SeqCst);
                        open_notify.notify_waiters();
                    }
                    ChannelEvent::Close => {
                        tracing::info!("logic_channel: channel closed");
                        is_open.store(false, Ordering::SeqCst);
                        break;
                    }
                    ChannelEvent::Message(data) => {
                        tracing::info!("logic_channel: received {} bytes", data.len());
                        let Ok(message) = bincode::deserialize(&data) else {
                            tracing::warn!("logic_channel: failed to deserialize message");
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
                                tracing::info!("logic_channel: forwarding message {:?}", message);
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
        let is_open = is_open.clone();
        let open_notify = open_notify.clone();
        async move {
            tracing::info!("logic_channel: sender waiting for channel to open");
            if !is_open.load(Ordering::SeqCst) {
                open_notify.notified().await;
            }
            tracing::info!("logic_channel: sender started");

            while let Some(control) = control_rx.recv().await {
                tracing::info!("logic_channel: sending {:?}", control);
                let Ok(encoded) = bincode::serialize(&control) else {
                    tracing::warn!("logic_channel: failed to serialize message");
                    continue;
                };
                tracing::info!("logic_channel: sending {} bytes", encoded.len());
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

    let chunk_size = std::env::var("VIDEO_CHUNK_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_VIDEO_CHUNK_SIZE);

    let (chunk_tx, mut chunk_rx) = chunk::<VideoMessage>(chunk_size).await?;
    let (assembly_tx, mut assembly_rx) = assembly::<VideoMessage>().await?;

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            while let Some(event) = assembly_rx.recv().await {
                match event {
                    AssemblyEvent::Whole(message) => {
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
        async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ChannelEvent::Open => {}
                    ChannelEvent::Close => {}
                    ChannelEvent::Message(data) => {
                        let Ok(chunk) = bincode::deserialize::<Chunk>(&data) else {
                            continue;
                        };
                        let _ = assembly_tx.send(AssemblyControl::Chunk(chunk)).await;
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
            while let Some(event) = chunk_rx.recv().await {
                match event {
                    ChunkEvent::Chunk(chunk) => {
                        let Ok(encoded) = bincode::serialize(&chunk) else {
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

    tokio::spawn({
        async move {
            while let Some(control) = control_rx.recv().await {
                match control {
                    VideoControl::Video(video) => {
                        let deadline = SystemTime::now() + VIDEO_FRAME_TTL;
                        let message = VideoMessage::Video(video);
                        let _ = chunk_tx.send(ChunkControl::Whole(message, deadline)).await;
                    }
                }
            }

            anyhow::Ok(())
        }
        .in_current_span()
    });

    Ok((control_tx, event_rx))
}
