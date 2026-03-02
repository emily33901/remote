use std::sync::Arc;

use anyhow::Result;
use crate::{Encoding, RateControlMode};
use crate::lifecycle::{run_stage, RunningStage, StageEvent};

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

#[cfg(target_os = "windows")]
use super::windows::{
    DesktopDuplicationCapture, DesktopDuplicationConfig,
    DxvaConverter, DxvaConverterConfig,
    MediaFoundationEncoder, MediaFoundationEncoderConfig,
    MediaFoundationDecoder, MediaFoundationDecoderConfig,
    D3D11Presenter, D3D11PresenterConfig,
};

#[cfg(target_os = "windows")]
pub struct SendPipeline {
    capture: RunningStage<DesktopDuplicationCapture>,
    converter: RunningStage<DxvaConverter>,
    encoder: RunningStage<MediaFoundationEncoder>,
}

#[cfg(target_os = "windows")]
impl SendPipeline {
    pub async fn new(config: SendPipelineConfig) -> Result<Self> {
        tracing::info!(
            width = config.width,
            height = config.height,
            frame_rate = config.frame_rate,
            encoding = ?config.encoding,
            "Creating send pipeline"
        );

        let capture = DesktopDuplicationCapture::new(DesktopDuplicationConfig {
            output_index: config.output_index,
        });

        let converter = DxvaConverter::new(DxvaConverterConfig {
            input_format: PixelFormat::BGRA,
            output_format: PixelFormat::NV12,
            output_width: config.width,
            output_height: config.height,
        });

        let encoder = MediaFoundationEncoder::new(MediaFoundationEncoderConfig {
            width: config.width,
            height: config.height,
            frame_rate: config.frame_rate,
            encoding: config.encoding,
            rate_control: config.rate_control,
        });

        let capture = run_stage(capture, 2).await?;
        let converter = run_stage(converter, 2).await?;
        let encoder = run_stage(encoder, 2).await?;

        Ok(Self {
            capture,
            converter,
            encoder,
        })
    }

    pub async fn start(&self) -> Result<()> {
        self.capture.send_start().await?;
        self.converter.send_start().await?;
        self.encoder.send_start().await?;
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        self.encoder.send_stop().await?;
        self.converter.send_stop().await?;
        self.capture.send_stop().await?;
        Ok(())
    }

    pub fn capture(&self) -> &RunningStage<DesktopDuplicationCapture> {
        &self.capture
    }

    pub fn converter(&self) -> &RunningStage<DxvaConverter> {
        &self.converter
    }

    pub fn encoder(&self) -> &RunningStage<MediaFoundationEncoder> {
        &self.encoder
    }
}

#[cfg(target_os = "windows")]
pub struct RecvPipeline {
    decoder: RunningStage<MediaFoundationDecoder>,
    presenter: RunningStage<D3D11Presenter>,
}

#[cfg(target_os = "windows")]
impl RecvPipeline {
    pub async fn new(config: RecvPipelineConfig) -> Result<Self> {
        tracing::info!(
            width = config.width,
            height = config.height,
            encoding = ?config.encoding,
            title = %config.title,
            "Creating receive pipeline"
        );

        let decoder = MediaFoundationDecoder::new(MediaFoundationDecoderConfig {
            width: config.width,
            height: config.height,
            encoding: config.encoding,
        });

        let presenter = D3D11Presenter::new(D3D11PresenterConfig {
            width: config.width,
            height: config.height,
            title: config.title,
        });

        let decoder = run_stage(decoder, 2).await?;
        let presenter = run_stage(presenter, 2).await?;

        Ok(Self {
            decoder,
            presenter,
        })
    }

    pub async fn start(&self) -> Result<()> {
        self.decoder.send_start().await?;
        self.presenter.send_start().await?;
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        self.presenter.send_stop().await?;
        self.decoder.send_stop().await?;
        Ok(())
    }

    pub fn decoder(&self) -> &RunningStage<MediaFoundationDecoder> {
        &self.decoder
    }

    pub fn presenter(&self) -> &RunningStage<D3D11Presenter> {
        &self.presenter
    }
}

#[cfg(not(target_os = "windows"))]
pub struct SendPipeline;

#[cfg(not(target_os = "windows"))]
impl SendPipeline {
    pub async fn new(_config: SendPipelineConfig) -> Result<Self> {
        anyhow::bail!("Send pipeline not implemented for this platform")
    }

    pub async fn start(&self) -> Result<()> {
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
pub struct RecvPipeline;

#[cfg(not(target_os = "windows"))]
impl RecvPipeline {
    pub async fn new(_config: RecvPipelineConfig) -> Result<Self> {
        anyhow::bail!("Receive pipeline not implemented for this platform")
    }

    pub async fn start(&self) -> Result<()> {
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        Ok(())
    }
}
