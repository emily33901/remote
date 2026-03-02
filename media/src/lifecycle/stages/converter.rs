use std::sync::Arc;

use async_trait::async_trait;
use anyhow::Result;

use crate::lifecycle::stage::{Stage, StageMeta};
use crate::lifecycle::state::AtomicLifecycleState;
use crate::texture_pool::Texture;
use crate::Timestamp;

#[derive(Debug, Clone, Copy)]
pub enum PixelFormat {
    BGRA,
    NV12,
    I420,
}

pub struct ConverterConfig {
    pub input_format: PixelFormat,
    pub output_format: PixelFormat,
    pub width: u32,
    pub height: u32,
}

pub struct ConverterInput {
    pub texture: Texture,
    pub timestamp: Timestamp,
}

pub struct ConverterOutput {
    pub texture: Texture,
    pub timestamp: Timestamp,
}

pub struct ConverterStage {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: ConverterConfig,
}

impl ConverterStage {
    pub fn new(config: ConverterConfig) -> Self {
        Self {
            meta: StageMeta::new("converter"),
            state: Arc::new(AtomicLifecycleState::default()),
            config,
        }
    }
}

#[async_trait]
impl Stage for ConverterStage {
    type Input = ConverterInput;
    type Output = ConverterOutput;

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
}
