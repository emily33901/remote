pub mod connection;
pub mod error;
pub mod peer;
pub mod protocol;
pub mod remote_peer;
pub mod types;

pub use error::{Error, Result};
pub use peer::{DisconnectReason, Peer, PeerConfig, PeerEvent, RemotePeerHandle, SignalMessage};
pub use protocol::{LogicMessage, Mode, StreamRequest, StreamRequestResponse};
pub use remote_peer::{DisconnectReason as RemoteDisconnectReason, RemotePeer, RemotePeerEvent, RemotePeerState};
pub use types::{ConnectionId, PeerId};

const ARBITRARY_CHANNEL_LIMIT: usize = 10;
