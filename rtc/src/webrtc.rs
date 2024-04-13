mod channel;
pub(crate) mod peer;

use std::sync::Arc;

use ::webrtc::{data_channel::RTCDataChannel, peer_connection::RTCPeerConnection};
use tokio::sync::mpsc;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;

use self::channel::ChannelStorage;

use super::{ChannelControl, ChannelEvent, ChannelOptions, DataChannel, PeerConnection};
use eyre::Result;

impl DataChannel for RTCDataChannel {}

pub(crate) struct WebrtcRsPeerConnection {
    inner: Arc<RTCPeerConnection>,
    storage: ChannelStorage,
}

impl WebrtcRsPeerConnection {}

#[async_trait::async_trait]
impl PeerConnection for WebrtcRsPeerConnection {
    async fn channel(
        self: Arc<Self>,
        our_label: &str,
        controlling: bool,
        channel_options: Option<ChannelOptions>,
    ) -> Result<(mpsc::Sender<ChannelControl>, mpsc::Receiver<ChannelEvent>)> {
        channel::channel(
            self.storage.clone(),
            self.inner.clone(),
            our_label,
            controlling,
            channel_options.map(|options| RTCDataChannelInit {
                ordered: options.ordered,
                max_retransmits: options.max_retransmits,
                ..Default::default()
            }),
        )
        .await
    }

    async fn offer(&self, controlling: bool) -> Result<()> {
        // TODO(emily): I feel like this is a little silly.
        if controlling {
            let offer = self.inner.create_offer(None).await?;
            tracing::debug!("made offer {offer:?}");
            self.inner.set_local_description(offer).await?;
        }

        Ok(())
    }
}
