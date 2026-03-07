use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Signaling error: {0}")]
    Signaling(#[from] SignalError),

    #[error("WebRTC error: {0}")]
    WebRTC(#[from] RtcError),

    #[error("Media error: {0}")]
    Media(#[from] MediaError),

    #[error("Peer not found: {0}")]
    PeerNotFound(crate::types::PeerId),

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Not connected")]
    NotConnected,

    #[error("Already connected")]
    AlreadyConnected,

    #[error("IO error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Error)]
pub enum SignalError {
    #[error("Failed to connect to signaling server: {0}")]
    ConnectionFailed(String),

    #[error("Signaling channel closed")]
    ChannelClosed,

    #[error("Invalid message: {0}")]
    InvalidMessage(String),
}

impl From<signal::SignallingError> for SignalError {
    fn from(err: signal::SignallingError) -> Self {
        match err {
            signal::SignallingError::NoSuchPeer(_) => {
                SignalError::InvalidMessage("no such peer".to_string())
            }
            signal::SignallingError::InternalError => {
                SignalError::ConnectionFailed("internal error".to_string())
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum RtcError {
    #[error("WebRTC connection failed: {0}")]
    ConnectionFailed(String),

    #[error("WebRTC channel closed")]
    ChannelClosed,

    #[error("ICE failed")]
    IceFailed,
}

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("Media encoding failed: {0}")]
    EncodingFailed(String),

    #[error("Media decoding failed: {0}")]
    DecodingFailed(String),

    #[error("Invalid video format")]
    InvalidFormat,
}

pub type Result<T> = std::result::Result<T, Error>;
