use std::sync::Arc;

use crate::ARBITRARY_CHANNEL_LIMIT;
use rtc::{self, ChannelControl, ChannelEvent, PeerConnection};
use tokio::sync::mpsc;

use eyre::Result;

pub(crate) enum AudioEvent {
    Audio(Vec<u8>),
}

pub(crate) enum AudioControl {
    Audio(Vec<u8>),
}

pub(crate) async fn audio_channel(
    peer_connection: Arc<dyn PeerConnection>,
    controlling: bool,
) -> Result<(mpsc::Sender<AudioControl>, mpsc::Receiver<AudioEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (tx, mut rx) = peer_connection.channel("audio", controlling, None).await?;

    telemetry::client::watch_channel(&control_tx, "audio-control").await;
    telemetry::client::watch_channel(&event_tx, "audio-event").await;

    tokio::spawn({
        let _tx = tx.clone();
        let event_tx = event_tx.clone();
        async move {
            match tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    match event {
                        ChannelEvent::Open => {}
                        ChannelEvent::Close => {}
                        ChannelEvent::Message(data) => {
                            event_tx.send(AudioEvent::Audio(data)).await?;
                        }
                    }
                }

                eyre::Ok(())
            })
            .await
            {
                Ok(r) => match r {
                    Ok(_) => {}
                    Err(err) => {
                        log::error!("audio channel event error {err}");
                    }
                },
                Err(err) => {
                    log::error!("audio channel event join error {err}");
                }
            }
        }
    });

    tokio::spawn({
        let tx = tx.clone();
        async move {
            match tokio::spawn(async move {
                while let Some(control) = control_rx.recv().await {
                    match control {
                        // TODO(emily): we should be pulling as much data as possible out of the
                        // channel here and passing to ChunkControl.
                        AudioControl::Audio(audio) => tx.send(ChannelControl::Send(audio)).await?,
                    }
                }

                eyre::Ok(())
            })
            .await
            {
                Ok(r) => match r {
                    Ok(_) => {}
                    Err(err) => {
                        log::error!("audio channel control error {err}");
                    }
                },
                Err(err) => {
                    log::error!("audio channel control join error {err}");
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}
