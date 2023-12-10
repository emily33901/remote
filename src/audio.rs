use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use webrtc::{data_channel::RTCDataChannel, peer_connection::RTCPeerConnection};

use crate::channel::{channel, ChannelControl, ChannelEvent};

use eyre::{eyre, Result};

pub(crate) enum AudioEvent {
    Audio(Vec<u8>),
}

pub(crate) enum AudioControl {
    Audio(Vec<u8>),
}

pub(crate) async fn audio_channel(
    peer_connection: Arc<RTCPeerConnection>,
    controlling: bool,
) -> Result<(mpsc::Sender<AudioControl>, mpsc::Receiver<AudioEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(10);
    let (event_tx, event_rx) = mpsc::channel(10);

    let (tx, mut rx) = channel(peer_connection, "audio", controlling, None).await?;

    let pending_audio: Arc<Mutex<Option<Vec<Vec<u8>>>>> = Arc::new(Mutex::new(Some(vec![])));

    tokio::spawn({
        let tx = tx.clone();
        let event_tx = event_tx.clone();
        async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ChannelEvent::Open(channel) => {
                        let pending_audio = pending_audio.lock().await.take().unwrap();
                        for audio in pending_audio {
                            tx.send(ChannelControl::Send(audio)).await.unwrap();
                        }
                    }
                    ChannelEvent::Close(channel) => {}
                    ChannelEvent::Message(channel, message) => {
                        event_tx
                            .send(AudioEvent::Audio(message.data.to_vec()))
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
                    AudioControl::Audio(audio) => {
                        tx.send(ChannelControl::Send(audio)).await.unwrap();
                    }
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}
