use crate::audio::audio_channel;
use crate::logic::logic_channel;
use crate::rtc::{self};
use crate::video::{video_channel, VideoBuffer};
use crate::{PeerId, ARBITRARY_CHANNEL_LIMIT};
use signal::SignallingControl;

use eyre::Result;
use tokio::sync::mpsc;

#[derive(Debug)]
pub(crate) enum PeerError {
    Unknown,
}

#[derive(Debug)]
pub(crate) enum PeerControl {
    Offer(String),
    Answer(String),
    IceCandidate(String),

    Audio(Vec<u8>),
    Video(VideoBuffer),

    Die,
}

#[derive(Debug)]
pub(crate) enum PeerEvent {
    Audio(Vec<u8>),
    Video(VideoBuffer),
    Error(PeerError),
}

pub(crate) async fn peer(
    api: rtc::Api,
    _our_peer_id: PeerId,
    their_peer_id: PeerId,
    signalling_control: mpsc::Sender<SignallingControl>,
    controlling: bool,
) -> Result<(mpsc::Sender<PeerControl>, mpsc::Receiver<PeerEvent>)> {
    let (peer_connection, rtc_control, mut rtc_event) = api.peer(controlling).await?;

    // TODO(emily): Funnelling audio + video through the same channel here creates a pinch point that can be less ideal.
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "peer-control").await;
    telemetry::client::watch_channel(&event_tx, "peer-event").await;

    logic_channel(peer_connection.clone(), controlling).await?;
    let (audio_tx, mut audio_rx) = audio_channel(peer_connection.clone(), controlling).await?;
    let (video_tx, mut video_rx) = video_channel(peer_connection.clone(), controlling).await?;

    tokio::spawn({
        let rtc_control = rtc_control.clone();
        let peer_connection = peer_connection.clone();
        async move {
            match tokio::spawn(async move {
                // keep peer connection alive
                let _peer_connection = peer_connection;

                while let Some(control) = control_rx.recv().await {
                    // log::debug!("peer control {control:?}");
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
                        PeerControl::Die => {
                            log::info!("peer control got die");
                            break;
                        }
                    }
                }
                eyre::Ok(())
            })
            .await
            {
                Ok(r) => match r {
                    Ok(_) => {
                        log::info!("peer control done")
                    }
                    Err(err) => {
                        log::error!("peer control error {err}");
                    }
                },
                Err(err) => {
                    log::error!("peer control join error {err}");
                }
            }
        }
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            match tokio::spawn(async move {
                while let Some(event) = audio_rx.recv().await {
                    match event {
                        crate::audio::AudioEvent::Audio(audio) => {
                            event_tx.send(PeerEvent::Audio(audio)).await?;
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
                        log::error!("audio rx error {err}");
                    }
                },
                Err(err) => {
                    log::error!("audio event join error {err}");
                }
            }
        }
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            match tokio::spawn(async move {
                while let Some(event) = video_rx.recv().await {
                    match event {
                        crate::video::VideoEvent::Video(video) => {
                            event_tx.send(PeerEvent::Video(video)).await?;
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
                        log::error!("video rx error {err}");
                    }
                },
                Err(err) => {
                    log::error!("video event join error {err}");
                }
            }
        }
    });

    tokio::spawn({
        let signalling_control = signalling_control.clone();
        let event_tx = event_tx.clone();
        async move {
            match tokio::spawn(async move {
                while let Some(event) = rtc_event.recv().await {
                    match event {
                        rtc::RtcPeerEvent::IceCandidate(candidate) => {
                            signalling_control
                                .send(SignallingControl::IceCandidate(their_peer_id, candidate))
                                .await?;
                        }
                        rtc::RtcPeerEvent::StateChange(state_change) => {
                            log::info!("peer state change: {state_change:?}");
                            if let rtc::RtcPeerState::Failed = state_change {
                                event_tx.send(PeerEvent::Error(PeerError::Unknown)).await?;
                                break;
                            }
                        }
                        rtc::RtcPeerEvent::Offer(offer) => {
                            signalling_control
                                .send(SignallingControl::Offer(their_peer_id, offer))
                                .await?;
                        }
                        rtc::RtcPeerEvent::Answer(answer) => {
                            signalling_control
                                .send(SignallingControl::Answer(their_peer_id, answer))
                                .await?;
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
                        log::error!("rtc_event error {err}");
                    }
                },
                Err(err) => {
                    log::error!("rtc_event join error {err}");
                }
            }
        }
    });

    peer_connection.offer(controlling).await?;

    Ok((control_tx, event_rx))
}
