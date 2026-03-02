use encoder::FrameIsKeyframe;
use serde::{Deserialize, Serialize};
pub use statistics::Statistics;

pub mod types;
pub mod traits;
pub mod platform;
pub mod gpu;
pub mod lifecycle;
pub mod pipeline;

pub use types::{Timestamp, VideoFrame, PixelFormat, FrameSize, CpuBuffer, EncodedPacket};
pub use traits::{Capture, VideoEncoder, VideoDecoder};

#[cfg(target_os = "windows")]
pub mod dx;

pub mod produce;

pub mod decoder;
pub mod encoder;

#[cfg(target_os = "windows")]
mod conversion;
#[cfg(target_os = "windows")]
pub mod desktop_duplication;
pub mod file_sink;
mod media_queue;
#[cfg(target_os = "windows")]
mod mf;
mod statistics;
#[cfg(target_os = "windows")]
mod texture_pool;
mod yuv_buffer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Encoding {
    H264,
    H265,
    AV1,
    VP9,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RateControlMode {
    Bitrate(u32),
    Quality(u32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct H264EncodingOptions {
    pub rate_control: RateControlMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct H2565EncodingOptions {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AV1EncodingOptions {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VP9EncodingOptions {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EncodingOptions {
    H264(H264EncodingOptions),
    H265(H2565EncodingOptions),
    AV1(AV1EncodingOptions),
    VP9(VP9EncodingOptions),
}

impl TryFrom<EncodingOptions> for H264EncodingOptions {
    type Error = anyhow::Error;

    fn try_from(value: EncodingOptions) -> Result<Self, Self::Error> {
        if let EncodingOptions::H264(options) = value {
            Ok(options)
        } else {
            Err(anyhow::anyhow!("Not H264 options"))
        }
    }
}

#[derive(Debug)]
pub enum SupportsEncodingOptions {
    Yes,
    // TODO(emily): Consider actually saying whats wrong instead of passing back a string
    No(String),
}

pub type Texture = texture_pool::Texture;

#[derive(Serialize, Deserialize, Clone)]
pub struct VideoBuffer {
    pub data: Vec<u8>,
    pub sequence_header: Option<Vec<u8>>,
    pub time: crate::Timestamp,
    pub duration: std::time::Duration,
    pub key_frame: FrameIsKeyframe,
    pub statistics: Statistics,
}

impl std::fmt::Debug for VideoBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoBuffer")
            .field("data", &self.data.len())
            .field("sequence_header", &self.sequence_header)
            .field("time", &self.time)
            .field("duration", &self.duration)
            .field("key_frame", &self.key_frame)
            .finish()
    }
}

const ARBITRARY_MEDIA_CHANNEL_LIMIT: usize = 1;
