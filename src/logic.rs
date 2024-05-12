use eyre::Result;
use media::{Encoding, EncodingOptions};
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

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PeerStreamRequest {
    pub(crate) preferred_mode: Option<Mode>,
    pub(crate) preferred_encoding: Option<Encoding>,
    pub(crate) preferred_encoding_options: Option<EncodingOptions>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum PeerStreamRequestResponse {
    /// Accept is 'this is what I am going to be sending to you'
    Accept {
        mode: Mode,
        encoding: Encoding,
        encoding_options: EncodingOptions,
    },
    /// Negotiate is 'here is what I can offer, pick one of these'
    Negotiate {
        viable_modes: Vec<Mode>,
        viable_encodings: Vec<Encoding>,
    },
    Reject,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum LogicMessage {
    StreamRequest(PeerStreamRequest),
    StreamRequestResponse(PeerStreamRequestResponse),
    StreamKeyframeRequest,
    Ping,
    Pong,
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
        let weak_control_tx = control_tx.downgrade();
        async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ChannelEvent::Open => {}
                    ChannelEvent::Close => {}
                    ChannelEvent::Message(data) => {
                        let message = bincode::deserialize(&data).unwrap();

                        match message {
                            LogicMessage::Ping => {
                                if let Some(tx) = weak_control_tx.upgrade() {
                                    let _ = tx.send(LogicMessage::Pong).await;
                                }
                            }
                            LogicMessage::Pong => {}

                            message => {
                                event_tx.send(message).await?;
                            }
                        }
                    }
                }
            }

            eyre::Ok(())
        }
        .in_current_span()
    });

    tokio::spawn({
        let tx = tx.clone();
        async move {
            while let Some(control) = control_rx.recv().await {
                let encoded = bincode::serialize(&control).unwrap();
                tx.send(ChannelControl::Send(encoded)).await?;
            }

            eyre::Ok(())
        }
        .in_current_span()
    });

    tokio::spawn({
        let weak_control_tx = control_tx.downgrade();
        async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                ticker.tick().await;
                if let Some(tx) = weak_control_tx.upgrade() {
                    let _ = tx.send(LogicMessage::Ping).await;
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}
