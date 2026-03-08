use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};
use rtc::{ChannelControl, ChannelEvent, ChannelOptions, PeerConnection};
use anyhow::Result;

pub struct MockPeerConnection {
    channels: Mutex<HashMap<String, MockChannel>>,
}

struct MockChannel {
    incoming_tx: mpsc::Sender<ChannelEvent>,
    outgoing_rx: Mutex<mpsc::Receiver<ChannelControl>>,
}

impl MockPeerConnection {
    pub fn new() -> Self {
        Self {
            channels: Mutex::new(HashMap::new()),
        }
    }

    pub async fn send_to_channel(&self, label: &str, data: Vec<u8>) {
        let channels = self.channels.lock().await;
        if let Some(ch) = channels.get(label) {
            let _ = ch.incoming_tx.send(ChannelEvent::Message(data)).await;
        }
    }

    pub async fn open_channel(&self, label: &str) {
        let channels = self.channels.lock().await;
        if let Some(ch) = channels.get(label) {
            let _ = ch.incoming_tx.send(ChannelEvent::Open).await;
        }
    }

    pub async fn close_channel(&self, label: &str) {
        let channels = self.channels.lock().await;
        if let Some(ch) = channels.get(label) {
            let _ = ch.incoming_tx.send(ChannelEvent::Close).await;
        }
    }

    pub async fn recv_outgoing(&self, label: &str) -> Option<ChannelControl> {
        let mut channels = self.channels.lock().await;
        if let Some(ch) = channels.get_mut(label) {
            ch.outgoing_rx.lock().await.recv().await
        } else {
            None
        }
    }
}

#[async_trait]
impl PeerConnection for MockPeerConnection {
    async fn channel(
        &self,
        label: &str,
        _controlling: bool,
        _options: Option<ChannelOptions>,
    ) -> Result<(mpsc::Sender<ChannelControl>, mpsc::Receiver<ChannelEvent>)> {
        let (outgoing_tx, outgoing_rx) = mpsc::channel(100);
        let (incoming_tx, incoming_rx) = mpsc::channel(100);

        self.channels.lock().await.insert(
            label.to_string(),
            MockChannel {
                incoming_tx,
                outgoing_rx: Mutex::new(outgoing_rx),
            },
        );

        Ok((outgoing_tx, incoming_rx))
    }

    async fn offer(&self, _controlling: bool) -> Result<()> {
        Ok(())
    }
}
