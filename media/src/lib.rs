use std::time::SystemTimeError;

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
mod texture_pool;
mod yuv_buffer;

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
