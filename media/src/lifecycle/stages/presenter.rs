use std::sync::Arc;

use async_trait::async_trait;
use anyhow::Result;

use crate::lifecycle::stage::{Stage, StageMeta};
use crate::lifecycle::state::AtomicLifecycleState;
use crate::texture_pool::Texture;
use crate::Timestamp;

pub struct PresenterConfig {
    pub width: u32,
    pub height: u32,
    pub name: String,
}

pub struct PresenterInput {
    pub texture: Arc<Texture>,
    pub timestamp: Timestamp,
}

pub struct PresenterStage {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: PresenterConfig,
}

impl PresenterStage {
    pub fn new(config: PresenterConfig) -> Self {
        Self {
            meta: StageMeta::new("presenter"),
            state: Arc::new(AtomicLifecycleState::default()),
            config,
        }
    }
}

#[async_trait]
impl Stage for PresenterStage {
    type Input = PresenterInput;
    type Output = ();

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
}
