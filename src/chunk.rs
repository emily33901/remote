use std::collections::{HashMap, HashSet};

use eyre::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Eq, Serialize, Deserialize)]
pub(crate) struct Chunk {
    data: Vec<u8>,
    id: u32,
    part: u32,
    total: u32,
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
    Whole(T),
    Chunk(Chunk),
}

pub(crate) enum ChunkEvent<T> {
    Whole(T),
    Chunk(Chunk),
}

pub(crate) async fn chunk<T: Serialize + for<'de> Deserialize<'de> + Send + 'static>(
    chunk_size: usize,
) -> Result<(mpsc::Sender<ChunkControl<T>>, mpsc::Receiver<ChunkEvent<T>>)> {
    let (control_tx, mut control_rx) = mpsc::channel(10);
    let (event_tx, event_rx) = mpsc::channel(10);

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            let mut chunk_arrangement: HashMap<u32, HashSet<Chunk>> = HashMap::new();
            let mut next_chunk_id = 0;

            while let Some(control) = control_rx.recv().await {
                match control {
                    ChunkControl::Whole(v) => {
                        let chunk_id: u32 = next_chunk_id;
                        next_chunk_id += 1;

                        let encoded: Vec<u8> = match bincode::serialize(&v) {
                            Ok(v) => v,
                            Err(err) => {
                                log::error!("!! failed to serialize v {err} {err:?}");
                                panic!()
                            }
                        };

                        log::debug!("!! chunking {chunk_id}");
                        let chunks = encoded.chunks(chunk_size);
                        let (total, _) = chunks.size_hint();

                        for (i, chunk) in chunks.enumerate() {
                            let chunk = Chunk {
                                // TODO(emily): SAD VEC COPY
                                data: chunk.to_vec(),
                                id: chunk_id,
                                part: i as u32,
                                total: total as u32,
                            };
                            log::debug!("!! chunk sending {chunk_id} {i} of {total}");
                            event_tx.send(ChunkEvent::Chunk(chunk)).await.unwrap();
                        }
                    }
                    ChunkControl::Chunk(chunk) => {
                        let total = chunk.total as usize;
                        let chunk_id = chunk.id;
                        let mut chunk_complete = false;

                        if let Some(chunks) = chunk_arrangement.get_mut(&chunk.id) {
                            log::debug!(
                                "!! chunk {chunk_id} part {} of {}",
                                chunk.part,
                                chunk.total
                            );

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
                                log::debug!("!! assembling {chunk_id}");
                                // got all chunks, build T
                                let mut data = vec![];
                                let mut chunks = chunks.drain().collect::<Vec<_>>();
                                chunks.sort_by_cached_key(|c| c.part);
                                // data.reserve();
                                for chunk in chunks {
                                    data.extend(chunk.data);
                                }
                                let v: T = bincode::deserialize(&data).unwrap();
                                event_tx.send(ChunkEvent::Whole(v)).await.unwrap();
                            }
                        }
                    }
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}
