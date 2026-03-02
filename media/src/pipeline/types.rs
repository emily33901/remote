use std::sync::Arc;

use crate::texture_pool::Texture;
use crate::{Statistics, Timestamp, VideoBuffer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    BGRA,
    NV12,
    I420,
    RGB24,
}

pub struct CaptureFrame {
    pub texture: Texture,
    pub timestamp: Timestamp,
}

pub struct ConvertFrame {
    pub texture: Texture,
    pub timestamp: Timestamp,
    pub format: PixelFormat,
}

pub struct EncodeFrame {
    pub texture: Texture,
    pub timestamp: Timestamp,
    pub statistics: Statistics,
}

pub struct EncodedData {
    pub buffer: VideoBuffer,
}

pub struct DecodeFrame {
    pub texture: Arc<Texture>,
    pub timestamp: Timestamp,
    pub statistics: Statistics,
}

pub struct PresentFrame {
    pub texture: Arc<Texture>,
    pub timestamp: Timestamp,
}
