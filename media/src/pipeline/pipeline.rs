use anyhow::Result;
use crate::{Encoding, RateControlMode};

use super::types::*;

#[derive(Debug, Clone)]
pub struct SendPipelineConfig {
    pub output_index: u32,
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub encoding: Encoding,
    pub rate_control: RateControlMode,
    pub use_hardware_encoder: bool,
}

impl Default for SendPipelineConfig {
    fn default() -> Self {
        Self {
            output_index: 0,
            width: 1920,
            height: 1080,
            frame_rate: 60,
            encoding: Encoding::H264,
            rate_control: RateControlMode::Quality(50),
            use_hardware_encoder: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecvPipelineConfig {
    pub width: u32,
    pub height: u32,
    pub encoding: Encoding,
    pub title: String,
}

impl Default for RecvPipelineConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            encoding: Encoding::H264,
            title: "Remote Display".to_string(),
        }
    }
}

pub struct SendPipeline;

impl SendPipeline {
    pub async fn new(config: SendPipelineConfig) -> Result<Self> {
        tracing::info!(
            width = config.width,
            height = config.height,
            frame_rate = config.frame_rate,
            encoding = ?config.encoding,
            "Creating send pipeline"
        );
        Ok(Self)
    }

    pub async fn start(&self) -> Result<()> {
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        Ok(())
    }
}

pub struct RecvPipeline;

impl RecvPipeline {
    pub async fn new(config: RecvPipelineConfig) -> Result<Self> {
        tracing::info!(
            width = config.width,
            height = config.height,
            encoding = ?config.encoding,
            title = %config.title,
            "Creating receive pipeline"
        );
        Ok(Self)
    }

    pub async fn start(&self) -> Result<()> {
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        Ok(())
    }
}
