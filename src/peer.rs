use std::sync::Arc;

use crate::audio::audio_channel;
use crate::logic::logic_channel;
use crate::signalling::SignallingControl;
use crate::PeerId;

use eyre::{eyre, Result};
use media_engine::MediaEngine;
use tokio::sync::mpsc;
use webrtc::api::interceptor_registry::*;
use webrtc::api::*;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::offer_answer_options::RTCOfferOptions;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

#[derive(Debug)]
pub(crate) enum PeerControl {
    Offer(String),
    Answer(String),
    IceCandidate(String),

    Audio(Vec<u8>),
    Video(Vec<u8>),
}

#[derive(Debug)]
pub(crate) enum PeerEvent {
    Audio(Vec<u8>),
    Video(Vec<u8>),
}

pub(crate) async fn peer(
    our_peer_id: PeerId,
    their_peer_id: PeerId,
    signalling_control: mpsc::Sender<SignallingControl>,
    controlling: bool,
) -> Result<(mpsc::Sender<PeerControl>, mpsc::Receiver<PeerEvent>)> {
    // Create a MediaEngine object to configure the supported codec
    let mut m = MediaEngine::default();

    // Register default codecs
    m.register_default_codecs()?;

    // Create a InterceptorRegistry. This is the user configurable RTP/RTCP Pipeline.
    // This provides NACKs, RTCP Reports and other features. If you use `webrtc.NewPeerConnection`
    // this is enabled by default. If you are manually managing You MUST create a InterceptorRegistry
    // for each PeerConnection.
    let mut registry = Registry::new();

    // Use the default set of Interceptors
    registry = register_default_interceptors(registry, &mut m)?;

    // Create the API object with the MediaEngine
    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();

    // Prepare the configuration
    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    };

    // Create a new RTCPeerConnection
    let peer_connection = Arc::new(api.new_peer_connection(config).await?);

    let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(1);

    // Set the handler for Peer connection state
    // This will notify you when the peer has connected/disconnected
    peer_connection.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        log::debug!("Peer Connection State has changed: {s}");

        if s == RTCPeerConnectionState::Failed {
            // Wait until PeerConnection has had no network activity for 30 seconds or another failure. It may be reconnected using an ICE Restart.
            // Use webrtc.PeerConnectionStateDisconnected if you are interested in detecting faster timeout.
            // Note that the PeerConnection may come back from PeerConnectionStateDisconnected.
            log::debug!("Peer Connection has gone to failed exiting");
            let _ = done_tx.try_send(());
        }

        Box::pin(async {})
    }));

    peer_connection.on_ice_candidate({
        let signalling_control = signalling_control.clone();
        let their_peer_id = their_peer_id;
        Box::new(move |c| {
            let signalling_control = signalling_control.clone();
            Box::pin(async move {
                if let Some(c) = c {
                    signalling_control
                        .send(SignallingControl::IceCandidate(
                            their_peer_id,
                            c.to_json().unwrap().candidate,
                        ))
                        .await
                        .unwrap();
                }
            })
        })
    });

    let (control_tx, mut control_rx) = mpsc::channel(10);
    let (event_tx, event_rx) = mpsc::channel(10);

    logic_channel(peer_connection.clone(), controlling).await?;
    let (audio_tx, mut audio_rx) = audio_channel(peer_connection.clone(), controlling).await?;

    tokio::spawn({
        let peer_connection = peer_connection.clone();
        let signalling_control = signalling_control.clone();
        async move {
            let (pending_candidates_tx, mut pending_candidates_rx) = mpsc::channel(10);

            while let Some(control) = control_rx.recv().await {
                // log::debug!("peer control {control:?}");
                match control {
                    PeerControl::Offer(offer) => {
                        peer_connection
                            .set_remote_description(RTCSessionDescription::offer(offer).unwrap())
                            .await
                            .unwrap();

                        let answer = peer_connection.create_answer(None).await.unwrap();

                        signalling_control
                            .send(SignallingControl::Answer(their_peer_id, answer.sdp.clone()))
                            .await
                            .unwrap();

                        peer_connection.set_local_description(answer).await.unwrap();

                        while let Ok(candidate) = pending_candidates_rx.try_recv() {
                            log::debug!("adding stored candidate");
                            peer_connection
                                .add_ice_candidate(RTCIceCandidateInit {
                                    candidate,
                                    ..Default::default()
                                })
                                .await
                                .unwrap();
                        }
                    }
                    PeerControl::Answer(answer) => {
                        peer_connection
                            .set_remote_description(RTCSessionDescription::answer(answer).unwrap())
                            .await
                            .unwrap();

                        while let Ok(candidate) = pending_candidates_rx.try_recv() {
                            log::debug!("adding stored candidate");
                            peer_connection
                                .add_ice_candidate(RTCIceCandidateInit {
                                    candidate,
                                    ..Default::default()
                                })
                                .await
                                .unwrap();
                        }
                    }
                    PeerControl::IceCandidate(ice_candidate) => {
                        if peer_connection.remote_description().await.is_some() {
                            peer_connection
                                .add_ice_candidate(RTCIceCandidateInit {
                                    candidate: ice_candidate,
                                    ..Default::default()
                                })
                                .await
                                .unwrap();
                        } else {
                            log::debug!("storing candidate until remote description arrives");
                            pending_candidates_tx.send(ice_candidate).await.unwrap();
                        }
                    }
                    PeerControl::Audio(audio) => {
                        audio_tx
                            .send(crate::audio::AudioControl::Audio(audio))
                            .await
                            .unwrap();
                    }
                    PeerControl::Video(video) => {
                        todo!()
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
                        log::debug!("audio event {}", audio.len());
                        event_tx.send(PeerEvent::Audio(audio)).await.unwrap();
                    }
                }
            }
        }
    });

    if controlling {
        let offer = peer_connection.create_offer(None).await?;

        log::debug!("made offer {offer:?}");

        signalling_control
            .send(SignallingControl::Offer(their_peer_id, offer.sdp.clone()))
            .await?;

        peer_connection.set_local_description(offer).await?;
    }

    Ok((control_tx, event_rx))
}
