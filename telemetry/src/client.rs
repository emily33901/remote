use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc},
};

use async_bincode::tokio::AsyncBincodeWriter;
use futures::{Sink, SinkExt};
use once_cell::sync::OnceCell;
use tokio::sync::{mpsc, Mutex};

use crate::{
    next_id, ChannelEvent, ChannelStatistic, CounterEvent, CounterStatistic, Id, TelemetryEvent,
};

static STATS_SINK: OnceCell<mpsc::Sender<TelemetryEvent>> = OnceCell::new();

async fn try_send_telemetry_event(event: TelemetryEvent) {
    if let Some(sink) = STATS_SINK.get() {
        let _ = sink.try_send(event);
    }
}

pub async fn watch_channel<T: Send + 'static>(sender: &mpsc::Sender<T>, name: &str) {
    let id = next_id();
    tokio::spawn({
        let sender = sender.downgrade();

        try_send_telemetry_event(TelemetryEvent::Channel(ChannelEvent::Open(
            id,
            name.to_owned(),
        )))
        .await;

        async move {
            match async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
                loop {
                    let _ = ticker.tick().await;
                    let stat = {
                        if let Some(sender) = sender.upgrade() {
                            ChannelStatistic {
                                id: id,
                                max_capacity: sender.max_capacity(),
                                capacity: sender.capacity(),
                            }
                        } else {
                            break;
                        }
                    };

                    try_send_telemetry_event(TelemetryEvent::Channel(ChannelEvent::Statistic(
                        stat,
                    )))
                    .await;
                }

                Ok::<_, eyre::Error>(())
            }
            .await
            {
                Ok(ok) => {}
                Err(err) => {
                    log::error!("watch_channel failed {err}");
                }
            }
        }
    });
}

#[derive(Default, Clone)]
pub struct Counter(Arc<AtomicUsize>);

impl Counter {
    pub fn update(&self, count: usize) {
        self.0.fetch_add(count, std::sync::atomic::Ordering::SeqCst);
    }

    fn get(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

pub async fn watch_counter(counter: &Counter, unit: crate::Unit, name: &str) {
    let id = next_id();
    tokio::spawn({
        let counter = Arc::downgrade(&counter.0);

        try_send_telemetry_event(TelemetryEvent::Counter(CounterEvent::New(
            id,
            unit,
            name.to_owned(),
        )))
        .await;

        async move {
            match async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
                loop {
                    let _ = ticker.tick().await;
                    let stat = {
                        if let Some(counter) = counter.upgrade() {
                            let counter = Counter(counter);
                            CounterStatistic {
                                id: id,
                                count: counter.get(),
                            }
                        } else {
                            break;
                        }
                    };

                    try_send_telemetry_event(TelemetryEvent::Counter(CounterEvent::Statistic(
                        stat,
                    )))
                    .await;
                }

                Ok::<_, eyre::Error>(())
            }
            .await
            {
                Ok(ok) => {}
                Err(err) => {
                    log::error!("watch_counter failed {err}");
                }
            }
        }
    });
}

pub async fn sink() {
    tokio::spawn(async move {
        log::info!("starting telemetry sink");
        let (tx, mut rx) = mpsc::channel(100);
        STATS_SINK.set(tx).unwrap();

        let rx: Arc<Mutex<mpsc::Receiver<TelemetryEvent>>> = Arc::new(Mutex::new(rx));

        loop {
            let rx = rx.clone();
            match async move {
                let stream = tokio::net::TcpStream::connect("[::1]:33901").await?;
                log::info!("telemetry connected");
                let mut bincode_writer = AsyncBincodeWriter::from(stream).for_async();

                while let Some(event) = rx.lock().await.recv().await {
                    bincode_writer.send(event).await?;
                }

                Ok::<_, eyre::Error>(())
            }
            .await
            {
                Ok(ok) => {}
                Err(err) => {
                    log::error!("telemetry sink went down {err}");
                }
            }
        }
    });
}
