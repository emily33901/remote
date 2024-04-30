use crate::audio::audio_channel;
use crate::logic::{self, logic_channel};
use crate::rtc::{self};
use crate::video::video_channel;
use crate::{PeerId, ARBITRARY_CHANNEL_LIMIT};
use media::VideoBuffer;
use signal::SignallingControl;

use eyre::Result;
use tokio::sync::mpsc;
use tracing::Instrument;

#[derive(Debug)]
pub(crate) enum PeerError {
    Unknown,
    /// Peer connection is closed or failed. (can never be restarted)
    Closed,
}

#[derive(Debug)]
pub(crate) enum PeerControl {
    Offer(String),
    Answer(String),
    IceCandidate(String),

    Audio(Vec<u8>),
    Video(VideoBuffer),

    RequestStream(logic::PeerStreamRequest),
    RequestStreamResponse(logic::PeerStreamRequestResponse),

    Die,
}

pub(crate) enum PeerEvent {
    StreamRequest(logic::PeerStreamRequest),
    RequestStreamResponse(logic::PeerStreamRequestResponse),

    Audio(Vec<u8>),
    Video(VideoBuffer),
    Error(PeerError),
}

impl std::fmt::Debug for PeerEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StreamRequest(arg0) => f.debug_tuple("StreamRequest").field(arg0).finish(),
            Self::RequestStreamResponse(arg0) => {
                f.debug_tuple("RequestStreamResponse").field(arg0).finish()
            }
            Self::Audio(arg0) => f.debug_tuple("Audio").field(&arg0.len()).finish(),
            Self::Video(arg0) => f.debug_tuple("Video").field(arg0).finish(),
            Self::Error(arg0) => f.debug_tuple("Error").field(arg0).finish(),
        }
    }
}

#[tracing::instrument(skip(api, signalling_control))]
pub(crate) async fn peer(
    api: rtc::Api,
    our_peer_id: PeerId,
    their_peer_id: PeerId,
    signalling_control: mpsc::Sender<SignallingControl>,
    controlling: bool,
) -> Result<(mpsc::Sender<PeerControl>, mpsc::Receiver<PeerEvent>)> {
    let (peer_connection, rtc_control, mut rtc_event) = api.peer(controlling).await?;

    // TODO(emily): Funnelling audio + video through the same channel here creates a pinch point that can be less ideal.
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (logic_tx, mut logic_rx) = logic_channel(peer_connection.as_ref(), controlling).await?;
    let (audio_tx, mut audio_rx) = audio_channel(peer_connection.as_ref(), controlling).await?;
    let (video_tx, mut video_rx) = video_channel(peer_connection.as_ref(), controlling).await?;

    tokio::spawn({
        let rtc_control = rtc_control.clone();
        let peer_connection = peer_connection.clone();
        let _our_peer_id = our_peer_id.clone();
        async move {
            match async move {
                // keep peer connection alive
                let _peer_connection = peer_connection;

                while let Some(control) = control_rx.recv().await {
                    // tracing::debug!("peer control {control:?}");
                    match control {
                        PeerControl::Offer(offer) => {
                            rtc_control.send(rtc::RtcPeerControl::Offer(offer)).await?;
                        }
                        PeerControl::Answer(answer) => {
                            rtc_control
                                .send(rtc::RtcPeerControl::Answer(answer))
                                .await?;
                        }
                        PeerControl::IceCandidate(ice_candidate) => {
                            rtc_control
                                .send(rtc::RtcPeerControl::IceCandidate(ice_candidate))
                                .await?;
                        }
                        PeerControl::Audio(audio) => {
                            audio_tx
                                .send(crate::audio::AudioControl::Audio(audio))
                                .await?;
                        }
                        PeerControl::Video(video) => {
                            video_tx
                                .send(crate::video::VideoControl::Video(video))
                                .await?;
                        }
                        PeerControl::RequestStream(request) => {
                            logic_tx
                                .send(crate::logic::LogicMessage::StreamRequest(request))
                                .await?;
                        }
                        PeerControl::RequestStreamResponse(response) => {
                            logic_tx
                                .send(crate::logic::LogicMessage::StreamRequestResponse(response))
                                .await?;
                        }
                        PeerControl::Die => {
                            tracing::info!("peer control got die");
                            break;
                        }
                    }
                }
                eyre::Ok(())
            }
            .await
            {
                Ok(_) => {
                    tracing::info!("peer control done")
                }
                Err(err) => {
                    tracing::error!("peer control error {err}");
                }
            }
        }
        .in_current_span()
    });

    tokio::spawn({
        let event_tx = event_tx.clone(); // .downgrade();
        let span = tracing::span!(tracing::Level::DEBUG, "AudioEvent");
        async move {
            match async move {
                while let Some(event) = audio_rx.recv().await {
                    match event {
                        crate::audio::AudioEvent::Audio(audio) => {
                            event_tx.send(PeerEvent::Audio(audio)).await?;
                        }
                    }
                }

                eyre::Ok(())
            }
            .await
            {
                Ok(_) => {}
                Err(err) => {
                    tracing::error!("audio rx error {err}");
                }
            }
        }
        .instrument(span)
        .in_current_span()
    });

    tokio::spawn({
        let event_tx = event_tx.clone(); // .downgrade();
        let span =
            tracing::span!(tracing::Level::DEBUG, "LogicEvent", %our_peer_id, %their_peer_id);
        async move {
            match async move {
                while let Some(event) = logic_rx.recv().await {
                    match event {
                        logic::LogicMessage::StreamRequest(request) => {
                            event_tx.send(PeerEvent::StreamRequest(request)).await?;
                        }
                        logic::LogicMessage::StreamRequestResponse(response) => {
                            event_tx
                                .send(PeerEvent::RequestStreamResponse(response))
                                .await?;
                        }
                    }
                }

                eyre::Ok(())
            }
            .await
            {
                Ok(_) => {}
                Err(err) => {
                    tracing::error!("audio rx error {err}");
                }
            }
        }
        .instrument(span)
        .in_current_span()
    });

    tokio::spawn({
        let event_tx = event_tx.clone(); // .downgrade();
        let span =
            tracing::span!(tracing::Level::DEBUG, "VideoEvent", %our_peer_id, %their_peer_id);
        async move {
            match async move {
                while let Some(event) = video_rx.recv().await {
                    match event {
                        crate::video::VideoEvent::Video(video) => {
                            event_tx.send(PeerEvent::Video(video)).await?;
                        }
                    }
                }

                eyre::Ok(())
            }
            .await
            {
                Ok(_) => {}
                Err(err) => {
                    tracing::error!("video rx error {err}");
                }
            }
        }
        .instrument(span)
        .in_current_span()
    });

    tokio::spawn({
        let our_peer_id = our_peer_id.clone();
        let their_peer_id = their_peer_id.clone();
        let signalling_control = signalling_control.clone();
        let event_tx = event_tx.downgrade();
        async move {
            match async move {
                while let Some(event) = rtc_event.recv().await {
                    match event {
                        rtc::RtcPeerEvent::IceCandidate(candidate) => {
                            signalling_control
                                .send(SignallingControl::IceCandidate(
                                    their_peer_id.clone(),
                                    candidate,
                                ))
                                .await?;
                        }
                        rtc::RtcPeerEvent::StateChange(state_change) => {
                            tracing::info!("peer state change: {state_change:?}");
                            if let rtc::RtcPeerState::Failed | rtc::RtcPeerState::Closed = state_change {
                                if let Some(event_tx) = event_tx.upgrade() {
                                    event_tx.send(PeerEvent::Error(PeerError::Closed)).await?;
                                } else {
                                    tracing::warn!(%our_peer_id, "state change failed but no event_tx to tell peer that it failed?");
                                }
                                break;
                            }
                        }
                        rtc::RtcPeerEvent::Offer(offer) => {
                            signalling_control
                                .send(SignallingControl::Offer(their_peer_id.clone(), offer))
                                .await?;
                        }
                        rtc::RtcPeerEvent::Answer(answer) => {
                            signalling_control
                                .send(SignallingControl::Answer(their_peer_id.clone(), answer))
                                .await?;
                        }
                    }
                }

                eyre::Ok(())
            }
            .await
            {
                Ok(_) => {}
                Err(err) => {
                    tracing::error!("rtc_event error {err}");
                }
            }
        }
        .in_current_span()
    });

    peer_connection.offer(controlling).await?;

    Ok((control_tx, event_rx))
}
