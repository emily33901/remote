pub mod client;
pub mod server;

use std::sync::atomic::AtomicUsize;

use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

pub type Id = usize;
pub type ClientId = usize;

// TODO(emily): Instad of New / Open, send an 'Info' event every couple seconds
// to inform server of names.

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Unit {
    Bytes,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TelemetryEvent {
    Channel(ChannelEvent),
    Counter(CounterEvent),
    New,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChannelStatistic {
    pub id: Id,
    pub max_capacity: usize,
    pub capacity: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CounterStatistic {
    pub id: Id,
    pub count: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum CounterEvent {
    New(Id, Unit, String),
    Statistic(CounterStatistic),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ChannelEvent {
    Open(Id, String),
    Statistic(ChannelStatistic),
    Close(Id),
}

static NEXT_ID: OnceCell<AtomicUsize> = OnceCell::new();

fn next_id() -> Id {
    let id = NEXT_ID.get_or_init(|| AtomicUsize::new(0));
    id.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}
