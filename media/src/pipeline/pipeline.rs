use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::{Encoding, RateControlMode};
use crate::lifecycle::{run_stage, StageEvent, StageControlHandle, AtomicLifecycleState};

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
    pub hwnd: isize,
    pub width: u32,
    pub height: u32,
    pub encoding: Encoding,
}

impl Default for RecvPipelineConfig {
    fn default() -> Self {
        Self {
            hwnd: 0,
            width: 1920,
            height: 1080,
            encoding: Encoding::H264,
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
    encoder_control: StageControlHandle<EncodeFrame>,
    event_rx: mpsc::Receiver<SendEvent>,
    state: Arc<AtomicLifecycleState>,
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

        let capture_stage = run_stage(capture, 2).await?;
        let converter_stage = run_stage(converter, 2).await?;
        let encoder_stage = run_stage(encoder, 2).await?;

        let (capture_control, capture_events, _capture_state) = capture_stage.split();
        let (converter_control, converter_events, _converter_state) = converter_stage.split();
        let (encoder_control, encoder_events, encoder_state) = encoder_stage.split();

        let (event_tx, event_rx) = mpsc::channel(16);

        let _capture_task = {
            let converter = converter_control.clone();
            let events = event_tx.clone();
            tokio::spawn(async move {
                let mut ev = capture_events;
                loop {
                    match ev.next().await {
                        Some(StageEvent::Output(frame)) => {
                            let convert_frame = ConvertFrame {
                                texture: frame.texture,
                                timestamp: frame.timestamp,
                                format: PixelFormat::NV12,
                            };
                            if converter.send_input(convert_frame).await.is_err() {
                                break;
                            }
                        }
                        Some(StageEvent::Error(e)) => {
                            let _ = events.send(SendEvent::Error(e)).await;
                        }
                        Some(StageEvent::FatalError(e)) => {
                            let _ = events.send(SendEvent::Error(e)).await;
                            break;
                        }
                        Some(StageEvent::Stopped) | None => break,
                        _ => {}
                    }
                }
            })
        };

        let _converter_task = {
            let encoder = encoder_control.clone();
            let events = event_tx.clone();
            tokio::spawn(async move {
                let mut ev = converter_events;
                loop {
                    match ev.next().await {
                        Some(StageEvent::Output(frame)) => {
                            let encode_frame = EncodeFrame {
                                texture: frame.texture,
                                timestamp: frame.timestamp,
                                statistics: crate::Statistics::default(),
                            };
                            if encoder.send_input(encode_frame).await.is_err() {
                                break;
                            }
                        }
                        Some(StageEvent::Error(e)) => {
                            let _ = events.send(SendEvent::Error(e)).await;
                        }
                        Some(StageEvent::FatalError(e)) => {
                            let _ = events.send(SendEvent::Error(e)).await;
                            break;
                        }
                        Some(StageEvent::Stopped) | None => break,
                        _ => {}
                    }
                }
            })
        };

        let _encoder_task = {
            let events = event_tx.clone();
            tokio::spawn(async move {
                let mut ev = encoder_events;
                loop {
                    match ev.next().await {
                        Some(StageEvent::Output(data)) => {
                            let _ = events.send(SendEvent::Output(data)).await;
                        }
                        Some(StageEvent::Error(e)) => {
                            let _ = events.send(SendEvent::Error(e)).await;
                        }
                        Some(StageEvent::FatalError(e)) => {
                            let _ = events.send(SendEvent::Error(e)).await;
                            break;
                        }
                        Some(StageEvent::Stopped) | None => {
                            let _ = events.send(SendEvent::Stopped).await;
                            break;
                        }
                        _ => {}
                    }
                }
            })
        };

        capture_control.send_start().await?;
        converter_control.send_start().await?;
        encoder_control.send_start().await?;

        Ok(Self {
            encoder_control,
            event_rx,
            state: encoder_state,
        })
    }

    pub async fn send(&self, control: SendControl) -> Result<()> {
        match control {
            SendControl::RequestKeyframe => {
                self.encoder_control.send_flush().await?;
            }
            SendControl::Stop => {
                self.encoder_control.send_stop().await?;
            }
        }
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<SendEvent> {
        self.event_rx.recv().await
    }
}

#[cfg(target_os = "windows")]
pub struct RecvPipeline {
    decoder_control: StageControlHandle<EncodedData>,
    event_rx: mpsc::Receiver<RecvEvent>,
}

#[cfg(target_os = "windows")]
impl RecvPipeline {
    pub async fn new(config: RecvPipelineConfig) -> Result<Self> {
        tracing::info!(
            width = config.width,
            height = config.height,
            encoding = ?config.encoding,
            "Creating receive pipeline"
        );

        let decoder = MediaFoundationDecoder::new(MediaFoundationDecoderConfig {
            width: config.width,
            height: config.height,
            encoding: config.encoding,
        });

        let presenter = D3D11Presenter::new(D3D11PresenterConfig {
            hwnd: config.hwnd,
            width: config.width,
            height: config.height,
        });

        let decoder_stage = run_stage(decoder, 2).await?;
        let presenter_stage = run_stage(presenter, 2).await?;

        let (decoder_control, decoder_events, _decoder_state) = decoder_stage.split();
        let (presenter_control, _, _presenter_state) = presenter_stage.split();

        let (event_tx, event_rx) = mpsc::channel(16);

        let presenter_for_task = presenter_control.clone();
        let events_for_task = event_tx.clone();
        tokio::spawn(async move {
            let mut ev = decoder_events;
            loop {
                match ev.next().await {
                    Some(StageEvent::Output(decode_frame)) => {
                        let present_frame = PresentFrame {
                            texture: decode_frame.texture,
                            timestamp: decode_frame.timestamp,
                        };
                        if presenter_for_task.send_input(present_frame).await.is_err() {
                            break;
                        }
                    }
                    Some(StageEvent::Error(e)) => {
                        let _ = events_for_task.send(RecvEvent::Error(e)).await;
                    }
                    Some(StageEvent::FatalError(e)) => {
                        let _ = events_for_task.send(RecvEvent::Error(e)).await;
                        break;
                    }
                    Some(StageEvent::Stopped) | None => {
                        let _ = events_for_task.send(RecvEvent::Stopped).await;
                        break;
                    }
                    _ => {}
                }
            }
        });

        decoder_control.send_start().await?;
        presenter_control.send_start().await?;

        Ok(Self {
            decoder_control,
            event_rx,
        })
    }

    pub async fn send(&self, control: RecvControl) -> Result<()> {
        match control {
            RecvControl::Data(data) => {
                self.decoder_control.send_input(data).await?;
            }
            RecvControl::Stop => {
                self.decoder_control.send_stop().await?;
            }
        }
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<RecvEvent> {
        self.event_rx.recv().await
    }
}

#[cfg(not(target_os = "windows"))]
pub struct SendPipeline;

#[cfg(not(target_os = "windows"))]
impl SendPipeline {
    pub async fn new(_config: SendPipelineConfig) -> Result<Self> {
        anyhow::bail!("Send pipeline not implemented for this platform")
    }

    pub async fn send(&self, _control: SendControl) -> Result<()> {
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<SendEvent> {
        None
    }
}

#[cfg(not(target_os = "windows"))]
pub struct RecvPipeline;

#[cfg(not(target_os = "windows"))]
impl RecvPipeline {
    pub async fn new(_config: RecvPipelineConfig) -> Result<Self> {
        anyhow::bail!("Receive pipeline not implemented for this platform")
    }

    pub async fn send(&self, _control: RecvControl) -> Result<()> {
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<RecvEvent> {
        None
    }
}
