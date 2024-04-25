use eyre::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;

use rtc::{ChannelControl, ChannelEvent, PeerConnection};

use crate::ARBITRARY_CHANNEL_LIMIT;

#[derive(Serialize, Deserialize)]
pub enum LogicEvent {
    StreamRequest,
}

#[derive(Serialize, Deserialize)]
pub enum LogicControl {
    RequestStream,
}

pub(crate) async fn logic_channel(
    peer_connection: &dyn PeerConnection,
    controlling: bool,
) -> Result<(mpsc::Sender<LogicControl>, mpsc::Receiver<LogicEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (tx, mut rx) = peer_connection.channel("logic", controlling, None).await?;

    tokio::spawn(async move {
        match async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ChannelEvent::Open => {}
                    ChannelEvent::Close => {}
                    ChannelEvent::Message(data) => {
                        let message = bincode::deserialize(&data).unwrap();
                        event_tx.send(message).await?;
                    }
                }
            }

            eyre::Ok(())
        }
        .await
        {
            Ok(_) => {}
            Err(err) => {
                tracing::error!("logic channel event error {err}");
            }
        }
    });

    tokio::spawn({
        let tx = tx.clone();
        async move {
            match tokio::spawn(async move {
                while let Some(control) = control_rx.recv().await {
                    let encoded = bincode::serialize(&control).unwrap();
                    tx.send(ChannelControl::Send(encoded)).await?;
                }

                eyre::Ok(())
            })
            .await
            {
                Ok(r) => match r {
                    Ok(_) => {}
                    Err(err) => {
                        tracing::error!("logic channel control error {err}");
                    }
                },
                Err(err) => {
                    tracing::error!("logic channel control join error {err}");
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}
