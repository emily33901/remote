use std::sync::{atomic::AtomicUsize, Arc};

use async_bincode::tokio::AsyncBincodeWriter;
use futures::{Sink, SinkExt};
use once_cell::sync::OnceCell;
use tokio::sync::{mpsc, Mutex};

use crate::{next_id, ChannelEvent, ChannelStatistics, TelemetryEvent};

static STATS_SINK: OnceCell<mpsc::Sender<TelemetryEvent>> = OnceCell::new();

async fn try_send_telemetry_event(event: TelemetryEvent) {
    if let Some(sink) = STATS_SINK.get() {
        sink.send(event).await.unwrap();
    }
}

pub async fn watch_channel<T: Send + 'static>(sender: &mpsc::Sender<T>, name: &str) {
    let id = next_id();
    tokio::spawn({
        let sender = sender.downgrade();

        try_send_telemetry_event(TelemetryEvent::ChannelEvent(ChannelEvent::Open(
            id,
            name.to_owned(),
        )))
        .await;

        async move {
            match async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
                loop {
                    let _ = ticker.tick().await;
                    let stats = {
                        if let Some(sender) = sender.upgrade() {
                            ChannelStatistics {
                                id: id,
                                max_capacity: sender.max_capacity(),
                                capacity: sender.capacity(),
                            }
                        } else {
                            break;
                        }
                    };

                    try_send_telemetry_event(TelemetryEvent::ChannelStatistics(stats)).await;
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
