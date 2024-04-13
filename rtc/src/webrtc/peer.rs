use std::sync::Arc;

use tokio::sync::mpsc;

use eyre::Result;
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors, media_engine::MediaEngine,
        setting_engine, APIBuilder,
    },
    ice_transport::{ice_candidate::RTCIceCandidateInit, ice_server::RTCIceServer},
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription, signaling_state::RTCSignalingState,
    },
};

use crate::{
    ARBITRARY_CHANNEL_LIMIT, {RtcPeerControl, RtcPeerEvent, RtcPeerState},
};

use super::{channel::ChannelStorage, WebrtcRsPeerConnection};

pub(crate) async fn rtc_peer(
    controlling: bool,
) -> Result<(
    Arc<dyn crate::PeerConnection>,
    mpsc::Sender<RtcPeerControl>,
    mpsc::Receiver<RtcPeerEvent>,
)> {
    let (control_tx, mut control_rx) = mpsc::channel::<RtcPeerControl>(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel::<RtcPeerEvent>(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "webrtc-rs-peer-control").await;
    telemetry::client::watch_channel(&event_tx, "webrtc-rs-peer-event").await;

    // Create a MediaEngine object to configure the supported codec
    let mut m = MediaEngine::default();

    // Register default codecs
    m.register_default_codecs()?;

    // Create a InterceptorRegistry. This is the user configurable RTP/RTCP Pipeline.
    // This provides NACKs, RTCP Reports and other features. If you use `webrtc.NewPeerConnection`
    // this is enabled by default. If you are manually managing You MUST create a InterceptorRegistry
    // for each PeerConnection.
    let mut registry = Registry::new();

    let setting_engine = setting_engine::SettingEngine::default();

    // Use the default set of Interceptors
    registry = register_default_interceptors(registry, &mut m)?;

    // Create the API object with the MediaEngine
    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .with_setting_engine(setting_engine)
        .build();

    // Prepare the configuration
    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec![
                "stun:stun.l.google.com:19302".to_owned(),
                "stun:stun.cloudflare.com:3478".to_owned(),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };

    // Create a new RTCPeerConnection
    let peer_connection = Arc::new(api.new_peer_connection(config).await?);

    // Set the handler for Peer connection state
    // This will notify you when the peer has connected/disconnected
    peer_connection.on_peer_connection_state_change({
        let event_tx = event_tx.clone();
        Box::new(move |s: RTCPeerConnectionState| {
            let event_tx = event_tx.clone();

            tracing::debug!("Peer Connection State has changed: {s}");

            if s == RTCPeerConnectionState::Failed {
                // Wait until PeerConnection has had no network activity for 30 seconds or another failure. It may be reconnected using an ICE Restart.
                // Use webrtc.PeerConnectionStateDisconnected if you are interested in detecting faster timeout.
                // Note that the PeerConnection may come back from PeerConnectionStateDisconnected.
                tracing::error!("Peer Connection has gone to failed exiting");
            }

            Box::pin(async move {
                event_tx
                    .send(RtcPeerEvent::StateChange(match s {
                        RTCPeerConnectionState::Unspecified => todo!(),
                        RTCPeerConnectionState::New => RtcPeerState::New,
                        RTCPeerConnectionState::Connecting => RtcPeerState::Connecting,
                        RTCPeerConnectionState::Connected => RtcPeerState::Connected,
                        RTCPeerConnectionState::Disconnected => RtcPeerState::Disconnected,
                        RTCPeerConnectionState::Failed => RtcPeerState::Failed,
                        RTCPeerConnectionState::Closed => RtcPeerState::Closed,
                    }))
                    .await
                    .unwrap()
            })
        })
    });

    peer_connection.on_signaling_state_change({
        let event_tx = event_tx.clone();
        let peer_connection = peer_connection.clone();
        Box::new(move |s: RTCSignalingState| {
            let event_tx = event_tx.clone();
            let peer_connection = peer_connection.clone();

            Box::pin(async move {
                if s == RTCSignalingState::HaveLocalOffer {
                    let offer = peer_connection.local_description().await.unwrap();
                    event_tx
                        .send(RtcPeerEvent::Offer(offer.sdp.clone()))
                        .await
                        .unwrap();
                }
            })
        })
    });

    peer_connection.on_ice_candidate({
        let event_tx = event_tx.clone();
        Box::new(move |c| {
            let event_tx = event_tx.clone();
            Box::pin(async move {
                if let Some(c) = c {
                    event_tx
                        .send(RtcPeerEvent::IceCandidate(c.to_json().unwrap().candidate))
                        .await
                        .unwrap();
                }
            })
        })
    });

    tokio::spawn({
        // TODO(emily): Keep weak here
        let peer_connection = Arc::downgrade(&peer_connection);
        let event_tx = event_tx.clone();
        async move {
            let (pending_candidates_tx, mut pending_candidates_rx) =
                mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

            while let Some(control) = control_rx.recv().await {
                if let Some(peer_connection) = peer_connection.upgrade() {
                    match control {
                        RtcPeerControl::IceCandidate(candidate) => {
                            if peer_connection.remote_description().await.is_some() {
                                peer_connection
                                    .add_ice_candidate(RTCIceCandidateInit {
                                        candidate: candidate,
                                        ..Default::default()
                                    })
                                    .await
                                    .unwrap();
                            } else {
                                tracing::debug!(
                                    "storing candidate until remote description arrives"
                                );
                                pending_candidates_tx.send(candidate).await.unwrap();
                            }
                        }
                        RtcPeerControl::Offer(offer) => {
                            peer_connection
                                .set_remote_description(
                                    RTCSessionDescription::offer(offer).unwrap(),
                                )
                                .await
                                .unwrap();

                            let answer = peer_connection.create_answer(None).await.unwrap();

                            event_tx
                                .send(RtcPeerEvent::Answer(answer.sdp.clone()))
                                .await
                                .unwrap();

                            peer_connection.set_local_description(answer).await.unwrap();

                            while let Ok(candidate) = pending_candidates_rx.try_recv() {
                                tracing::debug!("adding stored candidate");
                                peer_connection
                                    .add_ice_candidate(RTCIceCandidateInit {
                                        candidate,
                                        ..Default::default()
                                    })
                                    .await
                                    .unwrap();
                            }
                        }
                        RtcPeerControl::Answer(answer) => {
                            peer_connection
                                .set_remote_description(
                                    RTCSessionDescription::answer(answer).unwrap(),
                                )
                                .await
                                .unwrap();

                            while let Ok(candidate) = pending_candidates_rx.try_recv() {
                                tracing::debug!("adding stored candidate");
                                peer_connection
                                    .add_ice_candidate(RTCIceCandidateInit {
                                        candidate,
                                        ..Default::default()
                                    })
                                    .await
                                    .unwrap();
                            }
                        }
                    }
                } else {
                    break;
                }
            }
        }
    });

    let storage = ChannelStorage::default();

    let peer_connection = Arc::new(WebrtcRsPeerConnection {
        inner: peer_connection,
        storage: storage,
    });

    Ok((peer_connection, control_tx, event_rx))
}
