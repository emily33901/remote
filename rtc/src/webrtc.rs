mod channel;
pub(crate) mod peer;

use std::sync::Arc;

use ::webrtc::{data_channel::RTCDataChannel, peer_connection::RTCPeerConnection};
use tokio::sync::mpsc;
use webrtc::{
    data_channel::data_channel_init::RTCDataChannelInit,
    peer_connection::signaling_state::RTCSignalingState,
};

use self::channel::ChannelStorage;

use super::{ChannelControl, ChannelEvent, ChannelOptions, DataChannel, PeerConnection};
use eyre::Result;

impl DataChannel for RTCDataChannel {}

use derive_more::{Deref, DerefMut};

#[derive(Deref, DerefMut)]
struct RTCPeerConnectionHolder(webrtc::peer_connection::RTCPeerConnection);

impl Drop for RTCPeerConnectionHolder {
    fn drop(&mut self) {
        if self.signaling_state() != RTCSignalingState::Closed {
            tracing::error!(
                "RTCPeerConnectionHolder dropped before being closed (by dropping the control)"
            );
        }
    }
}
pub(crate) struct WebrtcRsPeerConnection {
    inner: Arc<RTCPeerConnectionHolder>,
    storage: ChannelStorage,
}

impl WebrtcRsPeerConnection {}

#[async_trait::async_trait]
impl PeerConnection for WebrtcRsPeerConnection {
    async fn channel(
        self: &Self,
        our_label: &str,
        controlling: bool,
        channel_options: Option<ChannelOptions>,
    ) -> Result<(mpsc::Sender<ChannelControl>, mpsc::Receiver<ChannelEvent>)> {
        channel::channel(
            self.storage.clone(),
            &self.inner,
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

impl Drop for WebrtcRsPeerConnection {
    fn drop(&mut self) {
        tracing::info!("WebrtcRsPeerConnection::drop");
    }
}
