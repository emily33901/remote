use std::sync::Arc;

use tokio::sync::mpsc;
use webrtc::peer_connection::RTCPeerConnection;

use crate::{
    rtc::{self, ChannelControl, ChannelEvent, ChannelStorage, PeerConnection},
    util, ARBITRARY_CHANNEL_LIMIT,
};

use eyre::Result;

pub(crate) enum AudioEvent {
    Audio(Vec<u8>),
}

pub(crate) enum AudioControl {
    Audio(Vec<u8>),
}

pub(crate) async fn audio_channel(
    channel_storage: ChannelStorage,
    peer_connection: Arc<dyn PeerConnection>,
    controlling: bool,
) -> Result<(mpsc::Sender<AudioControl>, mpsc::Receiver<AudioEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (tx, mut rx) = peer_connection
        .channel(channel_storage, "audio", controlling, None)
        .await?;

    telemetry::client::watch_channel(&control_tx, "audio-control").await;
    telemetry::client::watch_channel(&event_tx, "audio-event").await;

    tokio::spawn({
        let _tx = tx.clone();
        let event_tx = event_tx.clone();
        async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ChannelEvent::Open(_channel) => {}
                    ChannelEvent::Close(_channel) => {}
                    ChannelEvent::Message(_channel, data) => {
                        util::send(
                            "channel event to audio event",
                            &event_tx,
                            AudioEvent::Audio(data),
                        )
                        .await
                        .unwrap();
                    }
                }
            }
        }
    });

    tokio::spawn({
        let tx = tx.clone();
        async move {
            while let Some(control) = control_rx.recv().await {
                match control {
                    // TODO(emily): we should be pulling as much data as possible out of the
                    // channel here and passing to ChunkControl.
                    AudioControl::Audio(audio) => util::send(
                        "audio control to channel control",
                        &tx,
                        ChannelControl::Send(audio),
                    )
                    .await
                    .unwrap(),
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}
