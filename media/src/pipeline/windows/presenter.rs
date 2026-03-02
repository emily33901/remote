use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;

use crate::lifecycle::{Stage, StageMeta, AtomicLifecycleState, LifecycleState};

use crate::pipeline::traits::Presenter;
use crate::pipeline::types::PresentFrame;

#[derive(Debug, Clone)]
pub struct D3D11PresenterConfig {
    pub width: u32,
    pub height: u32,
    pub title: String,
}

pub struct D3D11Presenter {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: D3D11PresenterConfig,
    current_width: u32,
    current_height: u32,
}

impl D3D11Presenter {
    pub fn new(config: D3D11PresenterConfig) -> Self {
        let (width, height) = (config.width, config.height);
        Self {
            meta: StageMeta::new("d3d11-presenter"),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            config,
            current_width: width,
            current_height: height,
        }
    }
}

#[async_trait]
impl Stage for D3D11Presenter {
    type Input = PresentFrame;
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

    async fn on_start(&mut self) -> Result<()> {
        tracing::info!(
            width = self.config.width,
            height = self.config.height,
            title = %self.config.title,
            "D3D11 presenter starting"
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
        Err(anyhow!("D3D11 presenter not yet fully implemented"))
    }
}

impl Presenter for D3D11Presenter {
    fn dimensions(&self) -> (u32, u32) {
        (self.current_width, self.current_height)
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.current_width = width;
        self.current_height = height;
    }
}
