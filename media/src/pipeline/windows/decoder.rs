use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;

use crate::lifecycle::{Stage, StageMeta, AtomicLifecycleState, LifecycleState};
use crate::Encoding;

use crate::pipeline::traits::Decoder;
use crate::pipeline::types::{DecodeFrame, EncodedData};

#[derive(Debug, Clone)]
pub struct MediaFoundationDecoderConfig {
    pub width: u32,
    pub height: u32,
    pub encoding: Encoding,
}

pub struct MediaFoundationDecoder {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: MediaFoundationDecoderConfig,
}

impl MediaFoundationDecoder {
    pub fn new(config: MediaFoundationDecoderConfig) -> Self {
        Self {
            meta: StageMeta::new("media-foundation-decoder"),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            config,
        }
    }
}

#[async_trait]
impl Stage for MediaFoundationDecoder {
    type Input = EncodedData;
    type Output = DecodeFrame;

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
            encoding = ?self.config.encoding,
            "MediaFoundation decoder starting"
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
        Err(anyhow!("MediaFoundation decoder not yet fully implemented"))
    }
}

impl Decoder for MediaFoundationDecoder {
    fn encoding(&self) -> Encoding {
        self.config.encoding
    }
}

#[derive(Debug, Clone)]
pub struct OpenH264DecoderConfig {
    pub width: u32,
    pub height: u32,
}

pub struct OpenH264Decoder {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: OpenH264DecoderConfig,
}

impl OpenH264Decoder {
    pub fn new(config: OpenH264DecoderConfig) -> Self {
        Self {
            meta: StageMeta::new("openh264-decoder"),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            config,
        }
    }
}

#[async_trait]
impl Stage for OpenH264Decoder {
    type Input = EncodedData;
    type Output = DecodeFrame;

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
            "OpenH264 decoder starting"
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
        Err(anyhow!("OpenH264 decoder not yet fully implemented"))
    }
}

impl Decoder for OpenH264Decoder {
    fn encoding(&self) -> Encoding {
        Encoding::H264
    }
}
