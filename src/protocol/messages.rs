use media::{Encoding, EncodingOptions};
use serde::{Deserialize, Serialize};

use super::Mode;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct StreamRequest {
    pub preferred_mode: Option<Mode>,
    pub preferred_encoding: Option<Encoding>,
    pub preferred_encoding_options: Option<EncodingOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamRequestResponse {
    Accept {
        mode: Mode,
        encoding: Encoding,
        encoding_options: EncodingOptions,
    },
    Negotiate {
        viable_modes: Vec<Mode>,
        viable_encodings: Vec<Encoding>,
    },
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogicMessage {
    StreamRequest(StreamRequest),
    StreamRequestResponse(StreamRequestResponse),
    StreamKeyframeRequest,
    Ping,
    Pong,
}
