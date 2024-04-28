use eyre::Result;
use serde::{Deserialize, Serialize};

use tokio::sync::mpsc;
use tracing::Instrument;

use rtc::{ChannelControl, ChannelEvent, PeerConnection};

use crate::ARBITRARY_CHANNEL_LIMIT;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct Mode {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) refresh_rate: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PeerStreamRequest {
    pub(crate) preferred_mode: Option<Mode>,
    pub(crate) preferred_bitrate: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum PeerStreamRequestResponse {
    Accept { mode: Mode, bitrate: u32 },
    Negotiate { viable_modes: Vec<Mode> },
    Reject,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum LogicMessage {
    StreamRequest(PeerStreamRequest),
    StreamRequestResponse(PeerStreamRequestResponse),
}

#[tracing::instrument(skip(peer_connection))]
pub(crate) async fn logic_channel(
    peer_connection: &dyn PeerConnection,
    controlling: bool,
) -> Result<(mpsc::Sender<LogicMessage>, mpsc::Receiver<LogicMessage>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (tx, mut rx) = peer_connection.channel("logic", controlling, None).await?;

    tokio::spawn({
        let span = tracing::span!(tracing::Level::DEBUG, "ChannelEvent");
        async move {
            match async move {
                while let Some(event) = rx.recv().await {
                    match event {
                        ChannelEvent::Open => {}
                        ChannelEvent::Close => {}
                        ChannelEvent::Message(data) => {
                            let message = bincode::deserialize(&data).unwrap();

                            tracing::debug!(?message);
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
        }
        .instrument(span)
        .in_current_span()
    });

    tokio::spawn({
        let tx = tx.clone();
        let span = tracing::span!(tracing::Level::DEBUG, "ChannelControl");
        async move {
            match async move {
                while let Some(control) = control_rx.recv().await {
                    tracing::debug!(?control);
                    let encoded = bincode::serialize(&control).unwrap();
                    tx.send(ChannelControl::Send(encoded)).await?;
                }

                eyre::Ok(())
            }
            .await
            {
                Ok(_) => {}
                Err(err) => {
                    tracing::error!("logic channel control error {err}");
                }
            }
        }
        .instrument(span)
        .in_current_span()
    });

    Ok((control_tx, event_rx))
}
