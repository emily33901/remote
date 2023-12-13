pub mod client;
pub mod server;

use std::sync::atomic::AtomicUsize;

use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

pub type Id = usize;
pub type ClientId = usize;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TelemetryEvent {
    ChannelStatistics(ChannelStatistics),
    ChannelEvent(ChannelEvent),
    New,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChannelStatistics {
    pub id: Id,
    pub max_capacity: usize,
    pub capacity: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ChannelEvent {
    Open(Id, String),
    Close(Id),
}

static NEXT_ID: OnceCell<AtomicUsize> = OnceCell::new();

fn next_id() -> Id {
    let id = NEXT_ID.get_or_init(|| AtomicUsize::new(0));
    let mut current = id.load(std::sync::atomic::Ordering::SeqCst);
    loop {
        match id.compare_exchange(
            current,
            current + 1,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        ) {
            Ok(value) => return value,
            Err(value) => current = value,
        }
    }
}
