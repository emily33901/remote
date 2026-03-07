mod channels;

pub use channels::*;

use crate::protocol::LogicMessage;
use crate::types::PeerId;
use media::VideoBuffer;
use rtc::{RtcPeerControl, RtcPeerEvent};
use signal::SignallingControl;
use tokio::sync::mpsc;
use tracing::Instrument;

const CHANNEL_LIMIT: usize = 10;

pub async fn create_peer_connection(
    api: rtc::Api,
    our_peer_id: PeerId,
    their_peer_id: PeerId,
    signalling_control: mpsc::Sender<SignallingControl>,
    controlling: bool,
) -> anyhow::Result<(
    mpsc::Sender<PeerConnectionControl>,
    mpsc::Receiver<PeerConnectionEvent>,
)> {
    let (peer_connection, rtc_control, mut rtc_event) = api.peer(controlling).await?;

    let (control_tx, mut control_rx) = mpsc::channel(CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(CHANNEL_LIMIT);

    let (logic_tx, mut logic_rx) = channels::logic_channel(peer_connection.as_ref(), controlling).await?;
    let (audio_tx, mut audio_rx) = channels::audio_channel(peer_connection.as_ref(), controlling).await?;
    let (video_tx, mut video_rx) = channels::video_channel(peer_connection.as_ref(), controlling).await?;

    tokio::spawn({
        let rtc_control = rtc_control.clone();
        let peer_connection = peer_connection.clone();
        async move {
            while let Some(control) = control_rx.recv().await {
                match control {
                    PeerConnectionControl::Offer(offer) => {
                        let _ = rtc_control.send(RtcPeerControl::Offer(offer)).await;
                    }
                    PeerConnectionControl::Answer(answer) => {
                        let _ = rtc_control.send(RtcPeerControl::Answer(answer)).await;
                    }
                    PeerConnectionControl::IceCandidate(candidate) => {
                        let _ = rtc_control.send(RtcPeerControl::IceCandidate(candidate)).await;
                    }
                    PeerConnectionControl::Audio(audio) => {
                        let _ = audio_tx.send(channels::AudioControl::Audio(audio)).await;
                    }
                    PeerConnectionControl::Video(video) => {
                        let _ = video_tx.send(channels::VideoControl::Video(video)).await;
                    }
                    PeerConnectionControl::RequestStream(request) => {
                        let _ = logic_tx.send(LogicMessage::StreamRequest(request)).await;
                    }
                    PeerConnectionControl::RequestStreamResponse(response) => {
                        let _ = logic_tx.send(LogicMessage::StreamRequestResponse(response)).await;
                    }
                    PeerConnectionControl::Disconnect => {
                        break;
                    }
                }
            }
        }
        .in_current_span()
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            while let Some(audio) = audio_rx.recv().await {
                match audio {
                    channels::AudioEvent::Audio(data) => {
                        let _ = event_tx.send(PeerConnectionEvent::Audio(data)).await;
                    }
                }
            }
        }
        .in_current_span()
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            while let Some(video) = video_rx.recv().await {
                match video {
                    channels::VideoEvent::Video(data) => {
                        let _ = event_tx.send(PeerConnectionEvent::Video(data)).await;
                    }
                }
            }
        }
        .in_current_span()
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            while let Some(message) = logic_rx.recv().await {
                match message {
                    LogicMessage::StreamRequest(request) => {
                        let _ = event_tx.send(PeerConnectionEvent::StreamRequest(request)).await;
                    }
                    LogicMessage::StreamRequestResponse(response) => {
                        let _ = event_tx.send(PeerConnectionEvent::StreamResponse(response)).await;
                    }
                    LogicMessage::StreamKeyframeRequest => {
                        tracing::warn!("ignoring keyframe request");
                    }
                    LogicMessage::Ping | LogicMessage::Pong => {}
                }
            }
        }
        .in_current_span()
    });

    tokio::spawn({
        let their_peer_id = their_peer_id.clone();
        let signalling_control = signalling_control.clone();
        let event_tx = event_tx.downgrade();
        async move {
            while let Some(event) = rtc_event.recv().await {
                match event {
                    RtcPeerEvent::IceCandidate(candidate) => {
                        let _ = signalling_control
                            .send(SignallingControl::IceCandidate(
                                their_peer_id.clone(),
                                candidate,
                            ))
                            .await;
                    }
                    RtcPeerEvent::StateChange(state) => {
                        if matches!(state, rtc::RtcPeerState::Failed | rtc::RtcPeerState::Closed) {
                            if let Some(event_tx) = event_tx.upgrade() {
                                let _ = event_tx.send(PeerConnectionEvent::Closed).await;
                            }
                            break;
                        }
                    }
                    RtcPeerEvent::Offer(offer) => {
                        let _ = signalling_control
                            .send(SignallingControl::Offer(their_peer_id.clone(), offer))
                            .await;
                    }
                    RtcPeerEvent::Answer(answer) => {
                        let _ = signalling_control
                            .send(SignallingControl::Answer(their_peer_id.clone(), answer))
                            .await;
                    }
                }
            }
        }
        .in_current_span()
    });

    peer_connection.offer(controlling).await?;

    Ok((control_tx, event_rx))
}

#[derive(Debug)]
pub enum PeerConnectionControl {
    Offer(String),
    Answer(String),
    IceCandidate(String),
    Audio(Vec<u8>),
    Video(VideoBuffer),
    RequestStream(crate::protocol::StreamRequest),
    RequestStreamResponse(crate::protocol::StreamRequestResponse),
    Disconnect,
}

#[derive(Debug)]
pub enum PeerConnectionEvent {
    StreamRequest(crate::protocol::StreamRequest),
    StreamResponse(crate::protocol::StreamRequestResponse),
    Audio(Vec<u8>),
    Video(VideoBuffer),
    Error(PeerConnectionError),
    Closed,
}

#[derive(Debug)]
pub enum PeerConnectionError {
    Closed,
    Unknown,
}
