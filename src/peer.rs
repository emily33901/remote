use std::sync::Arc;

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
}

pub(crate) async fn peer(
    our_peer_id: PeerId,
    their_peer_id: PeerId,
    signalling_control: mpsc::Sender<SignallingControl>,
    controlling: bool,
) -> Result<mpsc::Sender<PeerControl>> {
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

    // Register data channel creation handling
    peer_connection
        .on_data_channel(Box::new(move |d: Arc<RTCDataChannel>| {
            let d_label = d.label().to_owned();
            let d_id = d.id();
            log::debug!("New DataChannel {d_label} {d_id}");

            // Register channel opening handling
            Box::pin(async move {
                let d2 = Arc::clone(&d);
                let d_label2 = d_label.clone();
                let d_id2 = d_id;
                d.on_close(Box::new(move || {
                    log::debug!("Data channel closed");
                    Box::pin(async {})
                }));

                d.on_open(Box::new(move || {
                    log::debug!("Data channel '{d_label2}'-'{d_id2}' open. Random messages will now be sent to any connected DataChannels every 5 seconds");

                    Box::pin(async move {
                        let mut result = Result::<usize>::Ok(0);
                        while result.is_ok() {
                            let timeout = tokio::time::sleep(std::time::Duration::from_secs(5));
                            tokio::pin!(timeout);

                            tokio::select! {
                                _ = timeout.as_mut() =>{
                                    let message = format!("{:?}", std::time::Instant::now());
                                    log::debug!("Sending '{message}'");
                                    result = d2.send_text(message).await.map_err(Into::into);
                                }
                            };
                        }
                    })
                }));

                // Register text message handling
                d.on_message(Box::new(move |msg: DataChannelMessage| {
                    let msg_str = String::from_utf8(msg.data.to_vec()).unwrap();
                    log::debug!("Message from DataChannel '{d_label}': '{msg_str}'");
                    Box::pin(async {})
                }));
            })
        }));

    if controlling {
        // Create a datachannel with label 'data'
        let data_channel = peer_connection.create_data_channel("data", None).await?;

        // Register channel opening handling
        let d1 = Arc::clone(&data_channel);
        data_channel.on_open(Box::new(move || {
            println!("Data channel '{}'-'{}' open. Random messages will now be sent to any connected DataChannels every 5 seconds", d1.label(), d1.id());

            let d2 = Arc::clone(&d1);
            Box::pin(async move {
                let mut result = Result::<usize>::Ok(0);
                while result.is_ok() {
                    let timeout = tokio::time::sleep(std::time::Duration::from_secs(5));
                    tokio::pin!(timeout);

                    tokio::select! {
                        _ = timeout.as_mut() =>{
                                    let message = format!("{:?}", std::time::Instant::now());
                            println!("Sending '{message}'");
                            result = d2.send_text(message).await.map_err(Into::into);
                        }
                    };
                }
            })
        }));
    }

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

    let (tx, mut rx) = mpsc::channel(10);

    tokio::spawn({
        let peer_connection = peer_connection.clone();
        let signalling_control = signalling_control.clone();
        async move {
            let (pending_candidates_tx, mut pending_candidates_rx) = mpsc::channel(10);

            while let Some(control) = rx.recv().await {
                log::debug!("peer control {control:?}");
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
                }
            }
            log::debug!("peer control going down");
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

    Ok(tx)
}
