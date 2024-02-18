use encoder::FrameIsKeyframe;
use serde::{Deserialize, Serialize};



pub mod dx;

pub mod produce;

pub mod decoder;
pub mod encoder;

mod color_conversion;
pub mod desktop_duplication;
pub mod file_sink;
mod mf;
mod yuv_buffer;



#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VideoBuffer {
    pub data: Vec<u8>,
    pub sequence_header: Option<Vec<u8>>,
    pub time: std::time::SystemTime,
    pub duration: std::time::Duration,
    pub key_frame: FrameIsKeyframe,
}

const ARBITRARY_CHANNEL_LIMIT: usize = 10;
