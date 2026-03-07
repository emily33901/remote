use crate::protocol::{StreamRequest, StreamRequestResponse};
use media::VideoBuffer;

#[derive(Debug, Clone)]
pub enum RemotePeerEvent {
    StateChanged { new_state: super::RemotePeerState },
    StreamRequested { request: StreamRequest },
    StreamResponse { response: StreamRequestResponse },
    VideoFrame { frame: VideoBuffer },
    AudioData { data: Vec<u8> },
    Disconnected { reason: super::DisconnectReason },
    Error { error: String },
}
