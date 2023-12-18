use std::{collections::HashMap, sync::Arc};

use tokio::sync::{mpsc, Mutex};
use webrtc::{
    data_channel::{
        data_channel_init::RTCDataChannelInit, data_channel_message::DataChannelMessage,
        RTCDataChannel,
    },
    peer_connection::RTCPeerConnection,
};

use eyre::Result;

use crate::{
    ARBITRARY_CHANNEL_LIMIT, {ChannelControl, ChannelEvent},
};

const BUFFERED_AMOUNT_LOW_THRESHOLD: usize = 500_000;
const MAX_BUFFERED_AMOUNT: usize = 1_000_000;

#[derive(derive_more::Deref, derive_more::DerefMut, Clone, Default)]
pub(crate) struct ChannelStorage(
    Arc<
        Mutex<
            HashMap<
                String,
                (
                    Arc<Mutex<Option<mpsc::Receiver<ChannelControl>>>>,
                    mpsc::Sender<ChannelEvent>,
                    mpsc::Sender<ChannelControl>,
                ),
            >,
        >,
    >,
);

async fn on_datachannel(
    channel: Arc<RTCDataChannel>,
    our_label: String,
    event_tx: mpsc::Sender<ChannelEvent>,
    control_rx: Arc<Mutex<Option<mpsc::Receiver<ChannelControl>>>>,
    control_tx: mpsc::Sender<ChannelControl>,
) -> Result<()> {
    assert_eq!(channel.label(), our_label);

    let _id: u16 = channel.id();

    channel.on_close({
        let our_label = our_label.clone();
        let channel = channel.clone();
        let event_tx = event_tx.clone();
        let control_tx = control_tx.clone();
        Box::new(move || {
            log::debug!("channel {our_label} closed");
            let channel = channel.clone();
            let event_tx = event_tx.clone();
            let control_tx = control_tx.clone();
            Box::pin(async move {
                event_tx.send(ChannelEvent::Close).await.unwrap();
                control_tx.send(ChannelControl::Close).await.unwrap();
            })
        })
    });

    // Use mpsc channel to send and receive a signal when more data can be sent
    let (more_can_be_sent, mut maybe_more_can_be_sent) = tokio::sync::mpsc::channel(1);

    channel.on_error(Box::new(move |err| {
        Box::pin(async move { log::error!("channel error {err}") })
    }));

    channel.on_open({
        let our_label = our_label.clone();
        let channel = channel.clone();
        let event_tx = event_tx.clone();
        let _control_rx_holder = control_rx.clone();
        Box::new(move || {
            let channel = channel.clone();
            Box::pin(async move {
                log::debug!("!! channel {our_label} open");
                event_tx.send(ChannelEvent::Open).await.unwrap();

                // NOTE(emily): Only start handling controls once the data channel is open.
                // This gives us natural back-pressure whilst we are waiting for the channel to open

                tokio::spawn({
                    let control_rx_holder = control_rx.clone();
                    let channel = channel.clone();
                    async move {
                        let mut control_rx = control_rx_holder
                            .lock()
                            .await
                            .take()
                            .expect("expected channel control");
                        log::debug!("!! took channel {our_label} control");

                        let sent_counter = telemetry::client::Counter::default();
                        telemetry::client::watch_counter(
                            &sent_counter,
                            telemetry::Unit::Bytes,
                            &format!("channel-{our_label}-sent"),
                        )
                        .await;

                        while let Some(control) = control_rx.recv().await {
                            match control {
                                ChannelControl::SendText(text) => {
                                    channel.send_text(text).await.unwrap();
                                }
                                ChannelControl::Send(data) => {
                                    let len = data.len();

                                    match channel.send(&bytes::Bytes::from(data)).await {
                                        Ok(_) => {
                                            sent_counter.update(len);
                                        }
                                        Err(err) => {
                                            log::warn!("channel {our_label} unable to send {err}");
                                        }
                                    }

                                    let buffered_total = len + channel.buffered_amount().await;

                                    if buffered_total > MAX_BUFFERED_AMOUNT {
                                        // Wait for the signal that more can be sent
                                        log::warn!(
                                            "!! buffered_total too large, waiting for low mark"
                                        );
                                        let _ = maybe_more_can_be_sent.recv().await;
                                    }
                                }
                                ChannelControl::Close => {
                                    channel.close().await.unwrap();
                                    *control_rx_holder.lock().await = Some(control_rx);
                                    break;
                                }
                            }
                        }
                    }
                });
            })
        })
    });

    channel.on_message({
        let channel = channel.clone();
        let event_tx = event_tx.clone();
        let our_label = our_label.clone();

        let recv_counter = telemetry::client::Counter::default();
        telemetry::client::watch_counter(
            &recv_counter,
            telemetry::Unit::Bytes,
            &format!("channel-{our_label}-recv"),
        )
        .await;

        Box::new(move |msg: DataChannelMessage| {
            log::debug!("channel {our_label} message");
            let channel = channel.clone();
            let event_tx = event_tx.clone();
            let our_label = our_label.clone();

            recv_counter.update(msg.data.len());

            Box::pin(async move {
                event_tx
                    .send(ChannelEvent::Message(msg.data.to_vec()))
                    .await
                    .unwrap();
            })
        })
    });

    channel
        .set_buffered_amount_low_threshold(BUFFERED_AMOUNT_LOW_THRESHOLD)
        .await;

    channel
        .on_buffered_amount_low(Box::new(move || {
            let more_can_be_sent = more_can_be_sent.clone();

            Box::pin(async move { more_can_be_sent.send(()).await.unwrap() })
        }))
        .await;

    Ok(())
}

pub(crate) async fn channel(
    storage: ChannelStorage,
    peer_connection: Arc<RTCPeerConnection>,
    our_label: &str,
    controlling: bool,
    channel_options: Option<RTCDataChannelInit>,
) -> Result<(mpsc::Sender<ChannelControl>, mpsc::Receiver<ChannelEvent>)> {
    let our_label = our_label.to_owned();
    let (control_tx, control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, &format!("channel-{our_label}-control")).await;
    telemetry::client::watch_channel(&event_tx, &format!("channel-{our_label}-event")).await;

    let control_rx = Arc::new(Mutex::new(Some(control_rx)));

    {
        storage.lock().await.insert(
            our_label.clone(),
            (control_rx.clone(), event_tx.clone(), control_tx.clone()),
        );
    }

    // Register data channel creation handling
    // TODO(emily): There is only one on_data_channel per peer you silly.
    peer_connection.on_data_channel({
        let storage = storage.clone();
        Box::new(move |d: Arc<RTCDataChannel>| {
            let channel_label = d.label().to_owned();
            let id = d.id();

            log::debug!("New DataChannel {} {id}", d.label());

            Box::pin({
                let storage = storage.clone();
                async move {
                    if let Some(storage) = storage.lock().await.get(&channel_label) {
                        let our_label = channel_label;
                        let control_rx = storage.0.clone();
                        let event_tx = storage.1.clone();
                        let control_tx = storage.2.clone();
                        on_datachannel(d, our_label, event_tx, control_rx, control_tx)
                            .await
                            .unwrap();
                    }
                }
            })
        })
    });

    if controlling {
        // Create a datachannel with label
        let data_channel = peer_connection
            .create_data_channel(&our_label, channel_options)
            .await?;

        on_datachannel(
            data_channel,
            our_label,
            event_tx.clone(),
            control_rx,
            control_tx.clone(),
        )
        .await?;
    }

    Ok((control_tx, event_rx))
}
