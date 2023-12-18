use std::{
    collections::HashMap,
    sync::{mpsc::channel, Arc, Weak},
};

use tokio::sync::{mpsc, oneshot, Mutex};

use async_datachannel::{PeerConnection, RtcConfig};
use datachannel::{DataChannelHandler, RtcPeerConnection};
use eyre::Result;

use crate::{
    RtcPeerState, ARBITRARY_CHANNEL_LIMIT,
    {ChannelControl, ChannelEvent, ChannelOptions, RtcPeerControl, RtcPeerEvent},
};

const BUFFERED_AMOUNT_LOW_THRESHOLD: usize = 500_000;
const MAX_BUFFERED_AMOUNT: usize = 1_000_000;

pub(crate) struct DCH {
    pub(crate) our_label: String,
    pub(crate) channel_rx: Option<oneshot::Receiver<Box<datachannel::RtcDataChannel<DCH>>>>,
    pub(crate) event_tx: mpsc::Sender<ChannelEvent>,
    pub(crate) control_tx: mpsc::Sender<ChannelControl>,
    pub(crate) control_rx_holder: Arc<Mutex<Option<mpsc::Receiver<ChannelControl>>>>,
    pub(crate) runtime: tokio::runtime::Handle,
    pub(crate) more_can_be_sent: Arc<Mutex<Option<mpsc::Receiver<()>>>>,
    pub(crate) more_can_be_sent_tx: mpsc::Sender<()>,

    pub(crate) recv_counter: telemetry::client::Counter,
}

impl DataChannelHandler for DCH {
    fn on_open(&mut self) {
        // NOTE(emily): Only start handling controls once the data channel is open.
        // This gives us natural back-pressure whilst we are waiting for the channel to open

        let control_rx_holder = self.control_rx_holder.clone();

        let our_label = self.our_label.clone();

        self.runtime.spawn({
            let recv_counter = self.recv_counter.clone();
            let control_rx_holder = control_rx_holder.clone();
            let channel_rx = self.channel_rx.take().unwrap();
            let event_tx = self.event_tx.clone();
            let more_can_be_sent_holder = self.more_can_be_sent.clone();

            async move {
                let mut channel = channel_rx.await.unwrap();

                channel
                    .set_buffered_amount_low_threshold(BUFFERED_AMOUNT_LOW_THRESHOLD)
                    .unwrap();

                event_tx.send(ChannelEvent::Open).await.unwrap();

                let mut control_rx = control_rx_holder
                    .lock()
                    .await
                    .take()
                    .expect("expected channel control");
                log::debug!("!! took channel {our_label} control");
                let mut more_can_be_sent = more_can_be_sent_holder
                    .lock()
                    .await
                    .take()
                    .expect("expected more_can_be_sent");

                let sent_counter = telemetry::client::Counter::default();
                telemetry::client::watch_counter(
                    &sent_counter,
                    telemetry::Unit::Bytes,
                    &format!("channel-{our_label}-sent"),
                )
                .await;

                telemetry::client::watch_counter(
                    &recv_counter,
                    telemetry::Unit::Bytes,
                    &format!("channel-{our_label}-recv"),
                )
                .await;

                while let Some(control) = control_rx.recv().await {
                    match control {
                        ChannelControl::SendText(text) => {
                            todo!("cannot SendText with libdatachannel");
                            // channel.send_text(text).await.unwrap();
                        }
                        ChannelControl::Send(data) => {
                            let len = data.len();

                            sent_counter.update(len);

                            channel = tokio::task::spawn_blocking(move || {
                                // TODO(emily): Don't unwrap here please thank you
                                channel.send(&bytes::Bytes::from(data)).unwrap();
                                channel
                            })
                            .await
                            .unwrap();

                            // match channel.send(&bytes::Bytes::from(data)).await {
                            //     Ok(_) => {
                            //         sent_counter.update(len);
                            //     }
                            //     Err(err) => {
                            //         log::warn!("channel {our_label} unable to send {err}");
                            //     }
                            // }

                            // TODO(emily): Wait for buffered amount here please

                            let buffered_total = len + channel.buffered_amount();

                            if buffered_total > MAX_BUFFERED_AMOUNT {
                                // Wait for the signal that more can be sent
                                log::warn!(
                                    "!! {our_label} buffered_total too large, waiting for low mark"
                                );
                                let _ = more_can_be_sent.recv().await;
                            }
                        }
                        ChannelControl::Close => {
                            todo!("cannot close datachannel channel");
                            // channel.close().await.unwrap();
                            *control_rx_holder.lock().await = Some(control_rx);
                            break;
                        }
                    }
                }
            }
        });
    }

    fn on_closed(&mut self) {
        log::warn!("channel {} closed", self.our_label);
    }

    fn on_error(&mut self, err: &str) {
        log::error!("channel {} closed", self.our_label);
    }

    fn on_message(&mut self, msg: &[u8]) {
        self.recv_counter.update(msg.len());

        self.event_tx
            .blocking_send(ChannelEvent::Message(msg.to_vec()))
            .unwrap();
    }

    fn on_buffered_amount_low(&mut self) {
        log::warn!("channel {} buffered amount low", self.our_label);

        self.runtime.spawn({
            let more_can_be_sent_tx = self.more_can_be_sent_tx.clone();
            async move {
                more_can_be_sent_tx.send(()).await.unwrap();
            }
        });
    }

    fn on_available(&mut self) {
        // log::warn!("channel {} buffered amount low", self.our_label);
    }
}

pub(crate) struct PCH {
    event_tx: mpsc::Sender<RtcPeerEvent>,
    storage: DatachannelStorage,
    runtime: tokio::runtime::Handle,
}

impl datachannel::PeerConnectionHandler for PCH {
    type DCH = DCH;

    fn data_channel_handler(&mut self, info: datachannel::DataChannelInfo) -> Self::DCH {
        let mut storage = self.storage.blocking_lock();
        let (_, channel_rx, control_rx, event_tx, control_tx) =
            storage.get_mut(&info.label).unwrap();

        let (more_can_be_sent_tx, more_can_be_sent_rx) = mpsc::channel(1);

        Self::DCH {
            our_label: info.label,
            channel_rx: channel_rx.take(),
            event_tx: event_tx.clone(),
            control_rx_holder: control_rx.clone(),
            control_tx: control_tx.clone(),
            runtime: self.runtime.clone(),
            recv_counter: Default::default(),
            more_can_be_sent_tx: more_can_be_sent_tx,
            more_can_be_sent: Arc::new(Mutex::new(Some(more_can_be_sent_rx))),
        }
    }

    fn on_description(&mut self, sess_desc: datachannel::SessionDescription) {
        let event = match sess_desc.sdp_type {
            datachannel::SdpType::Answer => {
                RtcPeerEvent::Answer(serde_json::to_string(&sess_desc).unwrap())
            }
            datachannel::SdpType::Offer => {
                RtcPeerEvent::Offer(serde_json::to_string(&sess_desc).unwrap())
            }
            datachannel::SdpType::Pranswer => todo!(),
            datachannel::SdpType::Rollback => todo!(),
        };
        self.runtime.spawn({
            let event_tx = self.event_tx.clone();
            async move { event_tx.send(event).await.unwrap() }
        });
    }

    fn on_candidate(&mut self, cand: datachannel::IceCandidate) {
        self.runtime.spawn({
            let event_tx = self.event_tx.clone();
            async move {
                event_tx
                    .send(RtcPeerEvent::IceCandidate(cand.candidate))
                    .await
                    .unwrap()
            }
        });
    }

    fn on_connection_state_change(&mut self, state: datachannel::ConnectionState) {
        let state = RtcPeerEvent::StateChange(match state {
            datachannel::ConnectionState::New => RtcPeerState::New,
            datachannel::ConnectionState::Connecting => RtcPeerState::Connecting,
            datachannel::ConnectionState::Connected => RtcPeerState::Connected,
            datachannel::ConnectionState::Disconnected => RtcPeerState::Disconnected,
            datachannel::ConnectionState::Failed => RtcPeerState::Failed,
            datachannel::ConnectionState::Closed => RtcPeerState::Closed,
        });

        self.runtime.spawn({
            let event_tx = self.event_tx.clone();
            async move { event_tx.send(state).await.unwrap() }
        });
    }

    fn on_gathering_state_change(&mut self, state: datachannel::GatheringState) {}

    fn on_signaling_state_change(&mut self, state: datachannel::SignalingState) {}

    fn on_data_channel(&mut self, data_channel: Box<datachannel::RtcDataChannel<Self::DCH>>) {
        let tx = self
            .storage
            .blocking_lock()
            .get_mut(&data_channel.label())
            .unwrap()
            .0
            .take()
            .unwrap();

        match tx.send(data_channel) {
            Ok(_) => {}
            Err(_) => panic!("rx died"),
        }
    }
}

#[derive(derive_more::Deref, derive_more::DerefMut, Clone, Default)]
pub(crate) struct DatachannelStorage(
    Arc<
        Mutex<
            HashMap<
                String,
                (
                    Option<oneshot::Sender<Box<datachannel::RtcDataChannel<DCH>>>>,
                    Option<oneshot::Receiver<Box<datachannel::RtcDataChannel<DCH>>>>,
                    Arc<Mutex<Option<mpsc::Receiver<ChannelControl>>>>,
                    mpsc::Sender<ChannelEvent>,
                    mpsc::Sender<ChannelControl>,
                ),
            >,
        >,
    >,
);

struct DatachannelPeerConnection {
    inner: Mutex<Box<RtcPeerConnection<PCH>>>,
    channel_storage: DatachannelStorage,
}

impl DatachannelPeerConnection {}

#[async_trait::async_trait]
impl crate::PeerConnection for DatachannelPeerConnection {
    async fn channel(
        self: Arc<Self>,
        our_label: &str,
        controlling: bool,
        channel_options: Option<ChannelOptions>,
    ) -> Result<(mpsc::Sender<ChannelControl>, mpsc::Receiver<ChannelEvent>)> {
        super::channel::channel(
            &mut *self.inner.lock().await,
            self.channel_storage.clone(),
            our_label,
            controlling,
            channel_options,
        )
        .await
    }

    async fn offer(&self, controlling: bool) -> Result<()> {
        Ok(())
    }
}

pub(crate) async fn rtc_peer(
    controlling: bool,
) -> Result<(
    Arc<dyn crate::PeerConnection>,
    mpsc::Sender<RtcPeerControl>,
    mpsc::Receiver<RtcPeerEvent>,
)> {
    let (control_tx, mut control_rx) = mpsc::channel::<RtcPeerControl>(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel::<RtcPeerEvent>(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "dc-peer-control").await;
    telemetry::client::watch_channel(&event_tx, "dc-peer-event").await;

    let ice_servers = vec!["stun:stun.l.google.com:19302"];
    let config = RtcConfig::new(&ice_servers);

    let storage = DatachannelStorage::default();

    let peer_connection = RtcPeerConnection::new(
        &config,
        PCH {
            event_tx: event_tx.clone(),
            storage: storage.clone(),
            runtime: tokio::runtime::Handle::current(),
        },
    )?;

    let peer_connection = Arc::new(DatachannelPeerConnection {
        inner: Mutex::new(peer_connection),
        channel_storage: storage,
    });

    tokio::spawn({
        let peer_connection = Arc::downgrade(&peer_connection);
        async move {
            while let Some(control) = control_rx.recv().await {
                if let Some(peer_connection) = peer_connection.upgrade() {
                    match control {
                        RtcPeerControl::IceCandidate(candidate) => peer_connection
                            .inner
                            .lock()
                            .await
                            .add_remote_candidate(&datachannel::IceCandidate {
                                candidate: candidate,
                                mid: String::new(),
                            })
                            .unwrap(),
                        RtcPeerControl::Offer(offer) => peer_connection
                            .inner
                            .lock()
                            .await
                            .set_remote_description(&serde_json::from_str(&offer).unwrap())
                            .unwrap(),
                        RtcPeerControl::Answer(answer) => peer_connection
                            .inner
                            .lock()
                            .await
                            .set_remote_description(&serde_json::from_str(&answer).unwrap())
                            .unwrap(),
                    }
                }
            }
        }
    });

    Ok((peer_connection, control_tx, event_rx))
}
