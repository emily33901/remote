use anyhow::Result;

use crate::types::{EncodedPacket, VideoFrame};

pub trait VideoDecoder: Send + Sync {
    fn decode(&mut self, packet: &EncodedPacket) -> Result<Option<VideoFrame>>;
    fn flush(&mut self) -> Result<Vec<VideoFrame>>;
}

pub trait VideoDecoderFactory: Send + Sync {
    type Decoder: VideoDecoder;

    fn create_decoder(&self, width: u32, height: u32) -> Result<Self::Decoder>;

    fn name(&self) -> &'static str;
    fn is_hardware(&self) -> bool;
}
