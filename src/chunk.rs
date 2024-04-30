use std::collections::{HashMap, HashSet};

use eyre::Result;
use serde::{Deserialize, Serialize};

use tokio::sync::mpsc;
use tracing::Instrument;

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

#[tracing::instrument]
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
            match async move {
                type ChunkArragement = HashMap<u32, HashSet<Chunk>>;
                let mut chunk_arrangement: ChunkArragement = HashMap::new();
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
                loop {
                    fn remove_elapsed_chunks(chunk_arrangement: &mut ChunkArragement) {
                        let mut remove_ids: Vec<u32> = vec![];
                        for (id, chunks) in chunk_arrangement.iter() {
                            for chunk in chunks.iter() {
                                // If any of the chunks expired then cull all of them
                                if let Ok(_) = chunk.deadline.elapsed() {
                                    remove_ids.push(*id);
                                    break;
                                }
                            }
                        }
                        tracing::trace!(
                            remove_ids = remove_ids.len(),
                            "removing unfinished packets that expired",
                        );
                        if remove_ids.len() > 0 {}
                        let mut chunks_removed = 0;
                        for id in remove_ids {
                            let cs = chunk_arrangement.remove(&id).unwrap();
                            chunks_removed += cs.len()
                        }
                        tracing::trace!(chunks_removed);
                    }

                    async fn handle_control<
                        T: Serialize + for<'de> Deserialize<'de> + Send + 'static,
                    >(
                        control: AssemblyControl,
                        chunk_arrangement: &mut ChunkArragement,
                        event_tx: &mpsc::Sender<AssemblyEvent<T>>,
                    ) -> Result<()> {
                        match control {
                            AssemblyControl::Chunk(chunk) => {
                                let total = chunk.total as usize;
                                let chunk_id = chunk.id;
                                let mut chunk_complete = false;

                                if let Ok(_) = chunk.deadline.elapsed() {
                                    // Ignore elapsed chunk
                                    tracing::trace!("ignoring elapsed chunk");
                                    return Ok(());
                                }

                                if let Some(chunks) = chunk_arrangement.get_mut(&chunk.id) {
                                    chunks.insert(chunk);

                                    chunk_complete = chunks.len() == total;
                                } else {
                                    if chunk.total == 1 {
                                        // early out because we have a whole packet from one chunk
                                        // TODO(emily): Copy paste from above
                                        let v: T = bincode::deserialize(&chunk.data)?;
                                        event_tx.send(AssemblyEvent::Whole(v)).await.map_err(
                                            |_err| {
                                                eyre::eyre!("Failed to decode reassembled packet")
                                            },
                                        )?;
                                    } else {
                                        let mut chunk_storage = HashSet::new();
                                        chunk_storage.insert(chunk);
                                        chunk_arrangement.insert(chunk_id, chunk_storage);
                                    }
                                }

                                if chunk_complete {
                                    let mut chunks = chunk_arrangement.remove(&chunk_id).unwrap();
                                    // got all chunks, build T
                                    let mut data = vec![];
                                    let mut chunks = chunks.drain().collect::<Vec<_>>();
                                    chunks.sort_by_cached_key(|c| c.part);
                                    for mut chunk in chunks {
                                        data.append(&mut chunk.data);
                                    }
                                    let v: T = bincode::deserialize(&data)?;
                                    event_tx.send(AssemblyEvent::Whole(v)).await.map_err(
                                        |_err| {
                                            eyre::eyre!("Failed to decode reassembled packet")
                                        },
                                    )?;
                                }
                            }
                        }

                        eyre::Ok(())
                    }

                    tokio::select! {
                        control = control_rx.recv() => {
                            match control {
                                Some(control) => handle_control(control, &mut chunk_arrangement, &event_tx).await?,
                                None => break,
                            }
                        }
                        _t = ticker.tick() => {
                            remove_elapsed_chunks(&mut chunk_arrangement);
                        }
                    }
                }
                eyre::Ok(())
            }
            .await
            {
                Ok(_r) => {}
                Err(err) => {
                    tracing::error!("assembly control error {err}");
                }
            }
        }.in_current_span()
    });

    Ok((control_tx, event_rx))
}

#[tracing::instrument(skip(chunk_size))]
pub(crate) async fn chunk<T: Serialize + for<'de> Deserialize<'de> + Send + 'static>(
    chunk_size: usize,
) -> Result<(mpsc::Sender<ChunkControl<T>>, mpsc::Receiver<ChunkEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            match async move {
                let mut next_chunk_id = 0;

                while let Some(control) = control_rx.recv().await {
                    match control {
                        ChunkControl::Whole(v, ttl) => {
                            let chunk_id: u32 = next_chunk_id;
                            next_chunk_id += 1;

                            let event_tx = event_tx.clone();
                            let deadline = ttl;

                            let encoded = bincode::serialize(&v)?;

                            let total = ((encoded.len()) / chunk_size)
                                + (if encoded.len() % chunk_size == 0 {
                                    0
                                } else {
                                    1
                                });

                            let chunks = encoded.chunks(chunk_size);

                            for (i, chunk) in chunks.enumerate() {
                                if let Ok(_) = deadline.elapsed() {
                                    // We actually timed out trying to send these chunks!
                                    // give up.
                                    tracing::warn!("{} expired during chunking", chunk_id);
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

                                event_tx.send(ChunkEvent::Chunk(chunk)).await?;

                                // match event_tx.try_send(ChunkEvent::Chunk(chunk)) {
                                //     Ok(_) => {}
                                //     Err(mpsc::error::TrySendError::Full(_)) => {
                                //         tracing::warn!("chunk control backpressured");
                                //         break;
                                //     }
                                //     Err(err) => {
                                //         tracing::warn!("chunk control closed");
                                //         return Err(eyre!("chunk control closed"));
                                //     }
                                // }
                            }
                        }
                    }
                }
                eyre::Ok(())
            }
            .await
            {
                Ok(_) => {}
                Err(err) => {
                    tracing::error!("assembly control error {err}");
                }
            }
        }
        .in_current_span()
    });

    Ok((control_tx, event_rx))
}
