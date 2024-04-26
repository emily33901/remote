use eyre::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::Instrument;

use rtc::{ChannelControl, ChannelEvent, PeerConnection};

use crate::ARBITRARY_CHANNEL_LIMIT;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PeerStreamRequest {}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum PeerStreamRequestResponse {
    Accept(),
    Reject(),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum LogicEvent {
    StreamRequest(PeerStreamRequest),
    StreamRequestResponse(PeerStreamRequestResponse),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum LogicControl {
    RequestStream(PeerStreamRequest),
    RequestStreamResponse(PeerStreamRequestResponse),
}

#[tracing::instrument(skip(peer_connection))]
pub(crate) async fn logic_channel(
    peer_connection: &dyn PeerConnection,
    controlling: bool,
) -> Result<(mpsc::Sender<LogicControl>, mpsc::Receiver<LogicEvent>)> {
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
                            let control = bincode::deserialize(&data).unwrap();

                            // TODO(emily): Should we even have different control / event here if we are just translating
                            // from one messsage to an indentical message
                            let event = match control {
                                LogicControl::RequestStream(request) => {
                                    LogicEvent::StreamRequest(request)
                                }
                                LogicControl::RequestStreamResponse(response) => {
                                    LogicEvent::StreamRequestResponse(response)
                                }
                            };

                            tracing::debug!(?event);
                            event_tx.send(event).await?;
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
