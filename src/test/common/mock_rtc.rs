use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use rtc::{ChannelControl, ChannelEvent, ChannelOptions, PeerConnection};
use tokio::sync::{mpsc, Mutex};

use crate::protocol::LogicMessage;

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
        let ch = channels.get(label).unwrap_or_else(|| panic!("channel {label} does not exist"));
        ch.incoming_tx
            .send(ChannelEvent::Message(data))
            .await
            .expect("failed to send");
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

    pub async fn recv_outgoing_skip_ping(&self, label: &str) -> Option<ChannelControl> {
        loop {
            let outgoing = self.recv_outgoing(label).await?;
            if let ChannelControl::Send(data) = &outgoing {
                if let Ok(LogicMessage::Ping) = bincode::deserialize(data) {
                    continue;
                }
            }
            return Some(outgoing);
        }
    }
}

pub async fn recv_logic_message_skip_pong(
    rx: &mut mpsc::Receiver<LogicMessage>,
) -> Option<LogicMessage> {
    loop {
        let msg = rx.recv().await?;
        if matches!(msg, LogicMessage::Pong) {
            continue;
        }
        return Some(msg);
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
