use eyre::{eyre, Result};
use std::sync::Arc;
use webrtc::{
    data_channel::{data_channel_message::DataChannelMessage, RTCDataChannel},
    peer_connection::RTCPeerConnection,
};

use crate::channel::{channel, ChannelEvent, ChannelStorage};

pub(crate) async fn logic_channel(
    channel_storage: ChannelStorage,
    peer_connection: Arc<RTCPeerConnection>,
    controlling: bool,
) -> Result<()> {
    // let (tx, mut rx) =
    //     channel(channel_storage, peer_connection, "logic", controlling, None).await?;

    // tokio::spawn(async move {
    //     while let Some(event) = rx.recv().await {
    //         match event {
    //             ChannelEvent::Open(channel) => {
    //                 let mut result = Result::<usize>::Ok(0);
    //                 while result.is_ok() {
    //                     let timeout = tokio::time::sleep(std::time::Duration::from_secs(5));
    //                     tokio::pin!(timeout);

    //                     tokio::select! {
    //                         _ = timeout.as_mut() =>{
    //                             let message = format!("{:?}", std::time::Instant::now());
    //                             log::debug!("Sending '{message}'");
    //                             result = channel.send_text(message).await.map_err(Into::into);
    //                         }
    //                     };
    //                 }
    //             }
    //             ChannelEvent::Close(channel) => {}
    //             ChannelEvent::Message(channel, message) => {}
    //         }
    //     }
    // });

    Ok(())
}
