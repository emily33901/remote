mod datachannel;
mod webrtc;

use std::{collections::HashMap, fmt::Display, str::FromStr, sync::Arc};

use tokio::sync::{mpsc, Mutex};

use eyre::Result;

const ARBITRARY_CHANNEL_LIMIT: usize = 10;

#[derive(Debug)]
pub enum RtcPeerState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

pub enum RtcPeerEvent {
    IceCandidate(String),
    StateChange(RtcPeerState),
    Offer(String),
    Answer(String),
}

pub enum RtcPeerControl {
    IceCandidate(String),
    Offer(String),
    Answer(String),
}

pub enum ChannelEvent {
    Open,
    Close,
    Message(Vec<u8>),
}

pub enum ChannelControl {
    SendText(String),
    Send(Vec<u8>),
    Close,
}

pub trait DataChannel: Send + Sync {}

#[async_trait::async_trait]
pub trait PeerConnection: Send + Sync {
    async fn channel(
        self: Arc<Self>,
        our_label: &str,
        controlling: bool,
        channel_options: Option<ChannelOptions>,
    ) -> Result<(mpsc::Sender<ChannelControl>, mpsc::Receiver<ChannelEvent>)>;

    async fn offer(&self, controlling: bool) -> Result<()>;
}

pub struct ChannelOptions {
    pub ordered: Option<bool>,
    pub max_retransmits: Option<u16>,
}

pub enum Api {
    WebrtcRs,
    DataChannel,
}

impl Api {
    pub async fn peer(
        &self,
        controlling: bool,
    ) -> Result<(
        Arc<dyn PeerConnection>,
        mpsc::Sender<RtcPeerControl>,
        mpsc::Receiver<RtcPeerEvent>,
    )> {
        match self {
            Self::WebrtcRs => self::webrtc::peer::rtc_peer(controlling).await,
            Self::DataChannel => self::datachannel::peer::rtc_peer(controlling).await,
        }
    }
}

#[derive(Debug)]
pub struct ApiParseError;

impl Display for ApiParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiParseError")?;
        Ok(())
    }
}

impl std::error::Error for ApiParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }

    fn description(&self) -> &str {
        "description() is deprecated; use Display"
    }

    fn cause(&self) -> Option<&dyn std::error::Error> {
        self.source()
    }
}

impl FromStr for Api {
    type Err = ApiParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "webrtc-rs" | "WebrtcRs" => Ok(Self::WebrtcRs),
            "datachannel" | "DataChannel" => Ok(Self::DataChannel),
            _ => Err(ApiParseError),
        }
    }
}
