use std::sync::Arc;

use datachannel::{DataChannelInit, Reliability, RtcPeerConnection};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::{
    ARBITRARY_CHANNEL_LIMIT, {ChannelControl, ChannelEvent, ChannelOptions},
};

use eyre::Result;

use super::peer::{DatachannelStorage, DCH, PCH};

pub(crate) async fn channel(
    peer_connection: &mut RtcPeerConnection<PCH>,
    storage: DatachannelStorage,
    our_label: &str,
    controlling: bool,
    channel_options: Option<ChannelOptions>,
) -> Result<(mpsc::Sender<ChannelControl>, mpsc::Receiver<ChannelEvent>)> {
    let our_label = our_label.to_owned();
    let (control_tx, control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, &format!("channel-{our_label}-control")).await;
    telemetry::client::watch_channel(&event_tx, &format!("channel-{our_label}-event")).await;

    let control_rx = Arc::new(Mutex::new(Some(control_rx)));

    let (channel_tx, channel_rx) = oneshot::channel();

    if controlling {
        let mut init = DataChannelInit::default();
        if let Some(channel_options) = channel_options {
            let mut reliability = Reliability::default();
            match channel_options.max_retransmits {
                Some(0) => {
                    reliability = reliability.unreliable();
                    reliability = reliability.max_retransmits(0);
                }
                Some(v) => reliability = reliability.max_retransmits(v),

                None => {}
            }

            if let Some(false) = channel_options.ordered {
                reliability = reliability.unordered();
            }

            tracing::info!("{our_label} reliability options are {reliability:?}");

            init = init.reliability(reliability);
        }

        let (more_can_be_sent_tx, more_can_be_sent_rx) = mpsc::channel(1);

        let channel = peer_connection
            .create_data_channel_ex(
                &our_label,
                DCH {
                    our_label: our_label.clone(),
                    channel_rx: Some(channel_rx),
                    event_tx,
                    control_tx: control_tx.clone(),
                    control_rx_holder: control_rx,
                    runtime: tokio::runtime::Handle::current(),
                    recv_counter: Default::default(),
                    more_can_be_sent_tx: more_can_be_sent_tx,
                    more_can_be_sent: Arc::new(Mutex::new(Some(more_can_be_sent_rx))),
                },
                &init,
            )
            .unwrap();

        match channel_tx.send(channel) {
            Ok(_) => {}
            Err(_) => panic!("Failed to send channel to handler"),
        }
    } else {
        storage.lock().await.insert(
            our_label,
            (
                Some(channel_tx),
                Some(channel_rx),
                control_rx.clone(),
                Some(event_tx.clone()),
                Some(control_tx.clone()),
            ),
        );
    }

    Ok((control_tx, event_rx))
}
