mod webrtc;

use std::{collections::HashMap, fmt::Display, str::FromStr, sync::Arc};

use tokio::sync::{mpsc, Mutex};

use crate::{signalling::SignallingControl, PeerId};

use eyre::Result;

#[derive(Debug)]
pub(crate) enum RtcPeerState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

pub(crate) enum RtcPeerEvent {
    IceCandidate(String),
    StateChange(RtcPeerState),
    Offer(String),
    Answer(String),
}

pub(crate) enum RtcPeerControl {
    IceCandidate(String),
    Offer(String),
    Answer(String),
}

pub(crate) enum ChannelEvent {
    Open(Arc<dyn DataChannel>),
    Close(Arc<dyn DataChannel>),
    Message(Arc<dyn DataChannel>, Vec<u8>),
}

pub(crate) enum ChannelControl {
    SendText(String),
    Send(Vec<u8>),
    Close,
}

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

pub(crate) trait DataChannel: Send + Sync {}

#[async_trait::async_trait]
pub(crate) trait PeerConnection: Send + Sync {
    async fn channel(
        self: Arc<Self>,
        storage: ChannelStorage,
        our_label: &str,
        controlling: bool,
        channel_options: Option<ChannelOptions>,
    ) -> Result<(mpsc::Sender<ChannelControl>, mpsc::Receiver<ChannelEvent>)>;

    async fn offer(&self, controlling: bool) -> Result<()>;
}

pub(crate) struct ChannelOptions {
    pub(crate) ordered: Option<bool>,
    pub(crate) max_retransmits: Option<u16>,
}

pub(crate) enum Api {
    WebrtcRs,
}

impl Api {
    pub(crate) async fn peer(
        &self,
        controlling: bool,
    ) -> Result<(
        Arc<dyn PeerConnection>,
        mpsc::Sender<RtcPeerControl>,
        mpsc::Receiver<RtcPeerEvent>,
    )> {
        match self {
            Self::WebrtcRs => self::webrtc::peer::rtc_peer(controlling).await,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ApiParseError;

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
            _ => Err(ApiParseError),
        }
    }
}
