use std::collections::{HashMap, HashSet};

use eyre::Result;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tokio::sync::mpsc;

use crate::ARBITRARY_CHANNEL_LIMIT;

#[derive(Eq, Serialize, Deserialize)]
pub(crate) struct Chunk {
    data: Vec<u8>,
    id: u32,
    part: u32,
    total: u32,
    deadline: std::time::SystemTime,
}

impl PartialEq for Chunk {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.part == other.part && self.total == other.total
    }
}

impl std::hash::Hash for Chunk {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.part.hash(state);
        self.total.hash(state);
    }
}

pub(crate) enum ChunkControl<T> {
    Whole(T, std::time::SystemTime),
}

pub(crate) enum ChunkEvent {
    Chunk(Chunk),
}

pub(crate) enum AssemblyControl {
    Chunk(Chunk),
}

pub(crate) enum AssemblyEvent<T> {
    Whole(T),
}

pub(crate) async fn assembly<T: Serialize + for<'de> Deserialize<'de> + Send + 'static>() -> Result<
    (
        mpsc::Sender<AssemblyControl>,
        mpsc::Receiver<AssemblyEvent<T>>,
    ),
> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "assembly-control").await;
    telemetry::client::watch_channel(&event_tx, "assembly-event").await;

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            type ChunkArragement = HashMap<u32, HashSet<Chunk>>;
            let mut chunk_arrangement: ChunkArragement = HashMap::new();
            loop {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));

                fn remove_elapsed_chunks(chunk_arrangement: &mut ChunkArragement) {
                    let mut remove_ids: Vec<u32> = vec![];
                    for (id, chunks) in chunk_arrangement.iter() {
                        for chunk in chunks.iter() {
                            if let Ok(_) = chunk.deadline.elapsed() {
                                remove_ids.push(*id);
                                break;
                            }
                        }
                    }
                    for id in remove_ids {
                        chunk_arrangement.remove(&id);
                    }
                }

                async fn handle_control<
                    T: Serialize + for<'de> Deserialize<'de> + Send + 'static,
                >(
                    control: AssemblyControl,
                    chunk_arrangement: &mut ChunkArragement,
                    event_tx: &mpsc::Sender<AssemblyEvent<T>>,
                ) {
                    match control {
                        AssemblyControl::Chunk(chunk) => {
                            let total = chunk.total as usize;
                            let chunk_id = chunk.id;
                            let mut chunk_complete = false;

                            if let Ok(_) = chunk.deadline.elapsed() {
                                // Ignore elapsed chunk
                                return;
                            }

                            if let Some(chunks) = chunk_arrangement.get_mut(&chunk.id) {
                                chunks.insert(chunk);

                                chunk_complete = chunks.len() == total;
                            } else {
                                let mut chunk_storage = HashSet::new();
                                let id = chunk.id;
                                chunk_storage.insert(chunk);
                                chunk_arrangement.insert(id, chunk_storage);
                            }

                            if chunk_complete {
                                if let Some(mut chunks) = chunk_arrangement.remove(&chunk_id) {
                                    // got all chunks, build T
                                    let mut data = vec![];
                                    let mut chunks = chunks.drain().collect::<Vec<_>>();
                                    chunks.sort_by_cached_key(|c| c.part);
                                    // data.reserve();
                                    for mut chunk in chunks {
                                        data.append(&mut chunk.data);
                                    }
                                    let v: T = bincode::deserialize(&data).unwrap();
                                    event_tx.send(AssemblyEvent::Whole(v)).await.unwrap();
                                }
                            }
                        }
                    }
                }

                tokio::select! {
                    control = control_rx.recv() => {
                        match control {
                            Some(control) => handle_control(control, &mut chunk_arrangement, &event_tx).await,
                            None => break,
                        }
                    }
                    _ = ticker.tick() => {
                        remove_elapsed_chunks(&mut chunk_arrangement)
                    }
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}

pub(crate) async fn chunk<T: Serialize + for<'de> Deserialize<'de> + Send + 'static>(
    chunk_size: usize,
) -> Result<(mpsc::Sender<ChunkControl<T>>, mpsc::Receiver<ChunkEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "chunk-control").await;
    telemetry::client::watch_channel(&event_tx, "chunk-event").await;

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            let mut next_chunk_id = 0;

            while let Some(control) = control_rx.recv().await {
                match control {
                    ChunkControl::Whole(v, ttl) => {
                        let chunk_id: u32 = next_chunk_id;
                        next_chunk_id += 1;

                        let event_tx = event_tx.clone();
                        let deadline = ttl;

                        let encoded: Vec<u8> = match bincode::serialize(&v) {
                            Ok(v) => v,
                            Err(err) => {
                                log::error!("!! failed to serialize v {err} {err:?}");
                                panic!()
                            }
                        };

                        let chunks = encoded.chunks(chunk_size);
                        let (total, _) = chunks.size_hint();

                        for (i, chunk) in chunks.enumerate() {
                            if let Ok(_) = deadline.elapsed() {
                                // We actually timed out trying to send these chunks!
                                // give up.
                                break;
                            }
                            let chunk = Chunk {
                                // TODO(emily): SAD VEC COPY
                                data: chunk.to_vec(),
                                id: chunk_id,
                                part: i as u32,
                                total: total as u32,
                                deadline,
                            };
                            event_tx.send(ChunkEvent::Chunk(chunk)).await.unwrap();
                        }
                    }
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}
