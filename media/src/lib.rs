use std::{collections::HashSet, time::SystemTimeError};

use encoder::FrameIsKeyframe;
use eyre::Error;
use serde::{Deserialize, Serialize};
pub use statistics::Statistics;

pub mod dx;

pub mod produce;

pub mod decoder;
pub mod encoder;

mod conversion;
pub mod desktop_duplication;
pub mod file_sink;
mod media_queue;
mod mf;
mod statistics;
mod texture_pool;
mod yuv_buffer;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Encoding {
    H264,
    H265,
    AV1,
    VP9,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    type Error = eyre::Report;

    fn try_from(value: EncodingOptions) -> Result<Self, Self::Error> {
        if let EncodingOptions::H264(options) = value {
            Ok(options)
        } else {
            Err(eyre::eyre!("Not H264 options"))
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Timestamp(std::time::Duration);

impl Timestamp {
    pub fn new(offset: std::time::Duration) -> Self {
        Self(offset)
    }

    pub fn new_hns(hns: i64) -> Self {
        Timestamp(std::time::Duration::from_nanos(100 * hns as u64))
    }

    pub fn new_millis(millis: u64) -> Self {
        Timestamp(std::time::Duration::from_millis(millis))
    }

    pub fn new_diff(
        start: std::time::SystemTime,
        now: std::time::SystemTime,
    ) -> Result<Self, SystemTimeError> {
        Ok(Self(now.duration_since(start)?))
    }

    pub fn new_diff_instant(start: std::time::Instant, now: std::time::Instant) -> Self {
        Self(now.duration_since(start))
    }

    pub fn hns(&self) -> i64 {
        (self.0.as_nanos() / 100) as i64
    }

    pub fn duration(&self) -> std::time::Duration {
        self.0
    }
}

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
