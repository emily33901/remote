use async_bincode::tokio::AsyncBincodeReader;
use tokio::sync::mpsc;

use crate::{next_id, ClientId, TelemetryEvent};

use futures::{Stream, StreamExt};

pub async fn stream() -> mpsc::Receiver<(ClientId, TelemetryEvent)> {
    let (tx, rx) = mpsc::channel(100);

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind("[::1]:33901").await.unwrap();

        while let Ok((client, addr)) = listener.accept().await {
            let id = next_id();
            tokio::spawn({
                let tx = tx.clone();
                async move {
                    match async move {
                        tx.send((id, TelemetryEvent::New)).await?;

                        let mut bincode_reader = AsyncBincodeReader::from(client);

                        while let Some(Ok(event)) = bincode_reader.next().await {
                            tx.send((id, event)).await?
                        }

                        Ok::<_, eyre::Error>(())
                    }
                    .await
                    {
                        Ok(ok) => {}
                        Err(err) => {
                            log::warn!("client {id} went down {}", err);
                        }
                    }
                }
            });
        }
    });

    rx
}
