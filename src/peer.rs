use std::sync::Arc;

use crate::audio::audio_channel;
use crate::logic::logic_channel;
use crate::rtc::{self, ChannelStorage};
use crate::signalling::SignallingControl;
use crate::video::{video_channel, VideoBuffer};
use crate::{PeerId, ARBITRARY_CHANNEL_LIMIT};

use eyre::Result;
use tokio::sync::mpsc;
use webrtc::api::*;

#[derive(Debug)]
pub(crate) enum PeerControl {
    Offer(String),
    Answer(String),
    IceCandidate(String),

    Audio(Vec<u8>),
    Video(VideoBuffer),
}

#[derive(Debug)]
pub(crate) enum PeerEvent {
    Audio(Vec<u8>),
    Video(VideoBuffer),
}

pub(crate) async fn peer(
    api: rtc::Api,
    _our_peer_id: PeerId,
    their_peer_id: PeerId,
    signalling_control: mpsc::Sender<SignallingControl>,
    controlling: bool,
) -> Result<(mpsc::Sender<PeerControl>, mpsc::Receiver<PeerEvent>)> {
    let (peer_connection, rtc_control, mut rtc_event) = api.peer(controlling).await?;

    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "peer-control").await;
    telemetry::client::watch_channel(&event_tx, "peer-event").await;

    let channel_storage = ChannelStorage::default();

    logic_channel(
        channel_storage.clone(),
        peer_connection.clone(),
        controlling,
    )
    .await?;
    let (audio_tx, mut audio_rx) = audio_channel(
        channel_storage.clone(),
        peer_connection.clone(),
        controlling,
    )
    .await?;
    let (video_tx, mut video_rx) = video_channel(
        channel_storage.clone(),
        peer_connection.clone(),
        controlling,
    )
    .await?;

    tokio::spawn({
        let rtc_control = rtc_control.clone();
        async move {
            while let Some(control) = control_rx.recv().await {
                // log::debug!("peer control {control:?}");
                match control {
                    PeerControl::Offer(offer) => {
                        rtc_control
                            .send(rtc::RtcPeerControl::Offer(offer))
                            .await
                            .unwrap();
                    }
                    PeerControl::Answer(answer) => {
                        rtc_control
                            .send(rtc::RtcPeerControl::Answer(answer))
                            .await
                            .unwrap();
                    }
                    PeerControl::IceCandidate(ice_candidate) => {
                        rtc_control
                            .send(rtc::RtcPeerControl::IceCandidate(ice_candidate))
                            .await
                            .unwrap();
                    }
                    PeerControl::Audio(audio) => {
                        audio_tx
                            .send(crate::audio::AudioControl::Audio(audio))
                            .await
                            .unwrap();
                    }
                    PeerControl::Video(video) => {
                        video_tx
                            .send(crate::video::VideoControl::Video(video))
                            .await
                            .unwrap();
                    }
                }
            }
            log::debug!("peer control going down");
        }
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            while let Some(event) = audio_rx.recv().await {
                match event {
                    crate::audio::AudioEvent::Audio(audio) => {
                        // log::debug!("audio event {}", audio.len());
                        event_tx.send(PeerEvent::Audio(audio)).await.unwrap();
                    }
                }
            }
        }
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            while let Some(event) = video_rx.recv().await {
                match event {
                    crate::video::VideoEvent::Video(video) => {
                        // log::debug!("video event {}", video.len());
                        event_tx.send(PeerEvent::Video(video)).await.unwrap();
                    }
                }
            }
        }
    });

    tokio::spawn({
        let signalling_control = signalling_control.clone();
        async move {
            while let Some(event) = rtc_event.recv().await {
                match event {
                    rtc::RtcPeerEvent::IceCandidate(candidate) => {
                        signalling_control
                            .send(SignallingControl::IceCandidate(their_peer_id, candidate))
                            .await
                            .unwrap();
                    }
                    rtc::RtcPeerEvent::StateChange(state_change) => {
                        log::info!("Peer state change: {state_change:?}")
                    }
                    rtc::RtcPeerEvent::Offer(offer) => {
                        signalling_control
                            .send(SignallingControl::Offer(their_peer_id, offer))
                            .await
                            .unwrap();
                    }
                    rtc::RtcPeerEvent::Answer(answer) => {
                        signalling_control
                            .send(SignallingControl::Answer(their_peer_id, answer))
                            .await
                            .unwrap();
                    }
                }
            }
        }
    });

    peer_connection.offer(controlling).await?;

    Ok((control_tx, event_rx))
}
