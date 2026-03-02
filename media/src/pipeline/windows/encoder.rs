use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;

use crate::lifecycle::{Stage, StageMeta, AtomicLifecycleState, LifecycleState};
use crate::{Encoding, RateControlMode};

use crate::pipeline::traits::Encoder;
use crate::pipeline::types::{EncodeFrame, EncodedData};

#[derive(Debug, Clone)]
pub struct MediaFoundationEncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub encoding: Encoding,
    pub rate_control: RateControlMode,
}

pub struct MediaFoundationEncoder {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: MediaFoundationEncoderConfig,
    pending_keyframe: bool,
    current_bitrate: Option<u32>,
}

impl MediaFoundationEncoder {
    pub fn new(config: MediaFoundationEncoderConfig) -> Self {
        Self {
            meta: StageMeta::new("media-foundation-encoder"),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            config,
            pending_keyframe: false,
            current_bitrate: None,
        }
    }
}

#[async_trait]
impl Stage for MediaFoundationEncoder {
    type Input = EncodeFrame;
    type Output = EncodedData;

    fn meta(&self) -> &StageMeta {
        &self.meta
    }

    fn atomic_state(&self) -> Arc<AtomicLifecycleState> {
        self.state.clone()
    }

    async fn initialize(&mut self) -> Result<()> {
        Ok(())
    }

    async fn on_start(&mut self) -> Result<()> {
        tracing::info!(
            width = self.config.width,
            height = self.config.height,
            frame_rate = self.config.frame_rate,
            encoding = ?self.config.encoding,
            rate_control = ?self.config.rate_control,
            "MediaFoundation encoder starting"
        );
        Ok(())
    }

    async fn on_stop(&mut self) -> Result<()> {
        Ok(())
    }

    async fn on_reset(&mut self) -> Result<()> {
        Ok(())
    }

    async fn process(&mut self, _input: Self::Input) -> Result<Option<Self::Output>> {
        Err(anyhow!("MediaFoundation encoder not yet fully implemented"))
    }
}

impl Encoder for MediaFoundationEncoder {
    fn encoding(&self) -> Encoding {
        self.config.encoding
    }

    fn set_bitrate(&mut self, bitrate: u32) {
        self.current_bitrate = Some(bitrate);
    }

    fn request_keyframe(&mut self) {
        self.pending_keyframe = true;
    }
}

#[derive(Debug, Clone)]
pub struct OpenH264EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub bitrate: u32,
}

pub struct OpenH264Encoder {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: OpenH264EncoderConfig,
    pending_keyframe: bool,
    current_bitrate: u32,
}

impl OpenH264Encoder {
    pub fn new(config: OpenH264EncoderConfig) -> Self {
        Self {
            meta: StageMeta::new("openh264-encoder"),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            current_bitrate: config.bitrate,
            config,
            pending_keyframe: false,
        }
    }
}

#[async_trait]
impl Stage for OpenH264Encoder {
    type Input = EncodeFrame;
    type Output = EncodedData;

    fn meta(&self) -> &StageMeta {
        &self.meta
    }

    fn atomic_state(&self) -> Arc<AtomicLifecycleState> {
        self.state.clone()
    }

    async fn initialize(&mut self) -> Result<()> {
        Ok(())
    }

    async fn on_start(&mut self) -> Result<()> {
        tracing::info!(
            width = self.config.width,
            height = self.config.height,
            frame_rate = self.config.frame_rate,
            bitrate = self.config.bitrate,
            "OpenH264 encoder starting"
        );
        Ok(())
    }

    async fn on_stop(&mut self) -> Result<()> {
        Ok(())
    }

    async fn on_reset(&mut self) -> Result<()> {
        Ok(())
    }

    async fn process(&mut self, _input: Self::Input) -> Result<Option<Self::Output>> {
        Err(anyhow!("OpenH264 encoder not yet fully implemented"))
    }
}

impl Encoder for OpenH264Encoder {
    fn encoding(&self) -> Encoding {
        Encoding::H264
    }

    fn set_bitrate(&mut self, bitrate: u32) {
        self.current_bitrate = bitrate;
    }

    fn request_keyframe(&mut self) {
        self.pending_keyframe = true;
    }
}
