use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use webrtc::{
    data_channel::{
        data_channel_init::RTCDataChannelInit, data_channel_message::DataChannelMessage,
        RTCDataChannel,
    },
    peer_connection::RTCPeerConnection,
};

use eyre::{eyre, Result};

pub enum ChannelEvent {
    Open(Arc<RTCDataChannel>),
    Close(Arc<RTCDataChannel>),
    Message(Arc<RTCDataChannel>, DataChannelMessage),
}

pub enum ChannelControl {
    SendText(String),
    Close,
}

async fn on_open(
    channel: Arc<RTCDataChannel>,
    our_label: String,
    event_tx: mpsc::Sender<ChannelEvent>,
    control_rx: Arc<Mutex<Option<mpsc::Receiver<ChannelControl>>>>,
) -> Result<()> {
    if channel.label() != our_label {
        return Ok(());
    }

    let id: u16 = channel.id();
    channel.on_close({
        let our_label = our_label.clone();
        Box::new(move || {
            log::debug!("channel {our_label} closed");
            Box::pin(async {})
        })
    });

    channel.on_open({
        let our_label = our_label.clone();
        let channel = channel.clone();
        let event_tx = event_tx.clone();
        let control_rx_holder = control_rx.clone();
        Box::new(move || {
            Box::pin(async move {
                log::debug!("channel {our_label} open");
                event_tx
                    .send(ChannelEvent::Open(channel.clone()))
                    .await
                    .unwrap();

                tokio::spawn({
                    let channel = channel.clone();
                    async move {
                        let mut control_rx = control_rx_holder
                            .lock()
                            .await
                            .take()
                            .expect("expected channel control");
                        while let Some(control) = control_rx.recv().await {
                            match control {
                                ChannelControl::SendText(text) => {
                                    channel.send_text(text).await.unwrap();
                                }
                                ChannelControl::Close => {
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
        Box::new(move |msg: DataChannelMessage| {
            log::debug!("channel {our_label} message {msg:?}");
            let channel = channel.clone();
            let event_tx = event_tx.clone();
            Box::pin(async move {
                event_tx
                    .send(ChannelEvent::Message(channel, msg))
                    .await
                    .unwrap();
            })
        })
    });

    Ok(())
}

pub(crate) async fn channel(
    peer_connection: Arc<RTCPeerConnection>,
    our_label: &str,
    controlling: bool,
    channel_options: Option<RTCDataChannelInit>,
) -> Result<(mpsc::Sender<ChannelControl>, mpsc::Receiver<ChannelEvent>)> {
    let our_label = our_label.to_owned();
    let (control_tx, control_rx) = mpsc::channel(10);
    let (event_tx, event_rx) = mpsc::channel(10);

    let control_rx = Arc::new(Mutex::new(Some(control_rx)));

    // Register data channel creation handling
    peer_connection.on_data_channel({
        let our_label = our_label.clone();
        let event_tx = event_tx.clone();
        let control_rx = control_rx.clone();
        Box::new(move |d: Arc<RTCDataChannel>| {
            let channel_label = d.label().to_owned();
            let id = d.id();

            log::debug!("New DataChannel {our_label} {id}");

            if channel_label == our_label {
                let our_label = our_label.clone();
                let event_tx = event_tx.clone();
                let control_rx = control_rx.clone();
                Box::pin(async move {
                    on_open(d, our_label, event_tx, control_rx).await.unwrap();
                })
            } else {
                Box::pin(async {})
            }
        })
    });

    if controlling {
        // Create a datachannel with label 'data'
        let data_channel = peer_connection
            .create_data_channel(&our_label, channel_options)
            .await?;
        on_open(data_channel, our_label, event_tx.clone(), control_rx).await?;
    }

    Ok((control_tx, event_rx))
}
