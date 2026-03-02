use std::sync::Arc;

use async_trait::async_trait;
use anyhow::Result;

use crate::lifecycle::stage::{Stage, StageMeta};
use crate::lifecycle::state::AtomicLifecycleState;
use crate::{VideoBuffer, Statistics};
use crate::texture_pool::Texture;
use crate::Timestamp;

pub struct DecoderConfig {
    pub width: u32,
    pub height: u32,
    pub target_framerate: u32,
}

pub struct DecoderInput {
    pub buffer: VideoBuffer,
}

pub struct DecoderOutput {
    pub texture: Texture,
    pub timestamp: Timestamp,
    pub statistics: Statistics,
}

pub struct DecoderStage {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: DecoderConfig,
}

impl DecoderStage {
    pub fn new(config: DecoderConfig) -> Self {
        Self {
            meta: StageMeta::new("decoder"),
            state: Arc::new(AtomicLifecycleState::default()),
            config,
        }
    }
}

#[async_trait]
impl Stage for DecoderStage {
    type Input = DecoderInput;
    type Output = DecoderOutput;

    fn meta(&self) -> &StageMeta {
        &self.meta
    }

    fn atomic_state(&self) -> Arc<AtomicLifecycleState> {
        self.state.clone()
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
