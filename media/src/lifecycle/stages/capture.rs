use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use anyhow::Result;
use parking_lot::Mutex;

use crate::lifecycle::stage::{Stage, StageMeta};
use crate::lifecycle::state::AtomicLifecycleState;
use crate::texture_pool::Texture;
use crate::Timestamp;

pub struct CaptureConfig {
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
}

pub struct CaptureOutput {
    pub texture: Texture,
    pub timestamp: Timestamp,
}

pub struct CaptureStage {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: CaptureConfig,
}

impl CaptureStage {
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            meta: StageMeta::new("capture"),
            state: Arc::new(AtomicLifecycleState::default()),
            config,
        }
    }
}

#[async_trait]
impl Stage for CaptureStage {
    type Input = ();
    type Output = CaptureOutput;

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

    async fn on_pause(&mut self) -> Result<()> {
        Ok(())
    }

    async fn on_resume(&mut self) -> Result<()> {
        Ok(())
    }
}
