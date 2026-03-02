use anyhow::Result;

use crate::types::{EncodedPacket, VideoFrame};

pub trait VideoEncoder: Send + Sync {
    fn encode(&mut self, frame: &VideoFrame, force_keyframe: bool) -> Result<EncodedPacket>;
    fn set_bitrate(&mut self, bitrate: u32) -> Result<()>;
    fn set_framerate(&mut self, fps: u32) -> Result<()>;
    fn flush(&mut self) -> Result<Vec<EncodedPacket>>;
}

pub trait VideoEncoderFactory: Send + Sync {
    type Encoder: VideoEncoder;

    fn create_encoder(
        &self,
        width: u32,
        height: u32,
        framerate: u32,
        bitrate: Option<u32>,
    ) -> Result<Self::Encoder>;

    fn name(&self) -> &'static str;
    fn is_hardware(&self) -> bool;
}
