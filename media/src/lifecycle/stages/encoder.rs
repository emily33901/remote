use std::sync::Arc;

use async_trait::async_trait;
use anyhow::Result;

use crate::lifecycle::stage::{Stage, StageMeta};
use crate::lifecycle::state::AtomicLifecycleState;
use crate::{Encoding, EncodingOptions, VideoBuffer, Statistics};
use crate::texture_pool::Texture;
use crate::Timestamp;

pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub encoding: Encoding,
    pub options: EncodingOptions,
}

pub struct EncoderInput {
    pub texture: Texture,
    pub timestamp: Timestamp,
    pub statistics: Statistics,
}

pub struct EncoderOutput {
    pub buffer: VideoBuffer,
}

pub struct EncoderStage {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: EncoderConfig,
}

impl EncoderStage {
    pub fn new(config: EncoderConfig) -> Self {
        Self {
            meta: StageMeta::new("encoder"),
            state: Arc::new(AtomicLifecycleState::default()),
            config,
        }
    }
}

#[async_trait]
impl Stage for EncoderStage {
    type Input = EncoderInput;
    type Output = EncoderOutput;

    fn meta(&self) -> &StageMeta {
        &self.meta
    }

    fn atomic_state(&self) -> &AtomicLifecycleState {
        &self.state
    }

    async fn initialize(&mut self) -> Result<()> {
        Ok(())
    }

    async fn process(&mut self, _input: Self::Input) -> Result<Option<Self::Output>> {
        Ok(None)
    }

    async fn on_start(&mut self) -> Result<()> {
        Ok(())
    }

    async fn on_stop(&mut self) -> Result<()> {
        Ok(())
    }

    async fn on_flush(&mut self) -> Result<()> {
        Ok(())
    }
}
