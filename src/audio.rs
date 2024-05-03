use crate::ARBITRARY_CHANNEL_LIMIT;
use rtc::{self, ChannelControl, ChannelEvent, PeerConnection};
use tokio::sync::mpsc;

use eyre::Result;
use tracing::Instrument;

pub(crate) enum AudioEvent {
    Audio(Vec<u8>),
}

pub(crate) enum AudioControl {
    Audio(Vec<u8>),
}

#[tracing::instrument(skip(peer_connection))]
pub(crate) async fn audio_channel(
    peer_connection: &dyn PeerConnection,
    controlling: bool,
) -> Result<(mpsc::Sender<AudioControl>, mpsc::Receiver<AudioEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (tx, mut rx) = peer_connection.channel("audio", controlling, None).await?;

    tokio::spawn({
        let _tx = tx.clone();
        let event_tx = event_tx.clone();
        let span = tracing::debug_span!("ChannelEvent");
        async move {
            match async move {
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
            }
            .await
            {
                Ok(_) => {}
                Err(err) => {
                    tracing::error!("audio channel event error {err}");
                }
            }
        }
        .instrument(span)
        .in_current_span()
    });

    tokio::spawn({
        let tx = tx.clone();
        let span = tracing::debug_span!("AudioControl");
        async move {
            match tokio::spawn(async move {
                while let Some(control) = control_rx.recv().await {
                    match control {
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
                        tracing::error!("audio channel control error {err}");
                    }
                },
                Err(err) => {
                    tracing::error!("audio channel control join error {err}");
                }
            }
        }
        .instrument(span)
        .in_current_span()
    });

    Ok((control_tx, event_rx))
}
