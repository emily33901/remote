use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::lifecycle::{Stage, StageMeta, AtomicLifecycleState, LifecycleState};
use crate::{Encoding, RateControlMode, encoder};

use crate::pipeline::traits::Encoder;
use crate::pipeline::types::{EncodeFrame, EncodedData};

#[derive(Debug, Clone)]
pub struct MediaFoundationEncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub encoding: Encoding,
    pub rate_control: RateControlMode,
}

pub struct MediaFoundationEncoder {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: MediaFoundationEncoderConfig,
    control_tx: Option<mpsc::Sender<encoder::EncoderControl>>,
    event_rx: Option<mpsc::Receiver<encoder::EncoderEvent>>,
}

impl MediaFoundationEncoder {
    pub fn new(config: MediaFoundationEncoderConfig) -> Self {
        Self {
            meta: StageMeta::new("media-foundation-encoder"),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            config,
            control_tx: None,
            event_rx: None,
        }
    }
}

#[async_trait]
impl Stage for MediaFoundationEncoder {
    type Input = EncodeFrame;
    type Output = EncodedData;

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
        if self.config.encoding != Encoding::H264 {
            return Err(anyhow!("Only H264 encoding is currently supported"));
        }

        let enc = encoder::Encoder::MediaFoundation;
        let (control_tx, event_rx) = enc.run(
            self.config.width,
            self.config.height,
            self.config.frame_rate,
            crate::Encoding::H264,
            crate::EncodingOptions::H264(crate::H264EncodingOptions {
                rate_control: self.config.rate_control,
            }),
        ).await?;

        self.control_tx = Some(control_tx);
        self.event_rx = Some(event_rx);

        tracing::info!(
            width = self.config.width,
            height = self.config.height,
            frame_rate = self.config.frame_rate,
            encoding = ?self.config.encoding,
            "MediaFoundation encoder started"
        );

        Ok(())
    }

    async fn on_stop(&mut self) -> Result<()> {
        self.control_tx = None;
        self.event_rx = None;
        Ok(())
    }

    async fn on_reset(&mut self) -> Result<()> {
        self.control_tx = None;
        self.event_rx = None;
        Ok(())
    }

    async fn process(&mut self, input: Self::Input) -> Result<Option<Self::Output>> {
        let control_tx = self.control_tx.as_ref()
            .ok_or_else(|| anyhow!("Encoder not initialized"))?;

        control_tx.send(encoder::EncoderControl::Frame(
            input.texture,
            input.timestamp,
            input.statistics,
        )).await?;

        let event_rx = self.event_rx.as_mut()
            .ok_or_else(|| anyhow!("Encoder not initialized"))?;

        match event_rx.try_recv() {
            Ok(encoder::EncoderEvent::Data(buffer)) => {
                Ok(Some(EncodedData { buffer }))
            }
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                Err(anyhow!("Encoder channel disconnected"))
            }
        }
    }
}

impl Encoder for MediaFoundationEncoder {
    fn encoding(&self) -> Encoding {
        self.config.encoding
    }

    fn set_bitrate(&mut self, _bitrate: u32) {
    }

    fn request_keyframe(&mut self) {
    }
}

#[derive(Debug, Clone)]
pub struct OpenH264EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub bitrate: u32,
}

pub struct OpenH264Encoder {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: OpenH264EncoderConfig,
    control_tx: Option<mpsc::Sender<encoder::EncoderControl>>,
    event_rx: Option<mpsc::Receiver<encoder::EncoderEvent>>,
}

impl OpenH264Encoder {
    pub fn new(config: OpenH264EncoderConfig) -> Self {
        Self {
            meta: StageMeta::new("openh264-encoder"),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            config,
            control_tx: None,
            event_rx: None,
        }
    }
}

#[async_trait]
impl Stage for OpenH264Encoder {
    type Input = EncodeFrame;
    type Output = EncodedData;

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
        let enc = encoder::Encoder::OpenH264;
        let (control_tx, event_rx) = enc.run(
            self.config.width,
            self.config.height,
            self.config.frame_rate,
            crate::Encoding::H264,
            crate::EncodingOptions::H264(crate::H264EncodingOptions {
                rate_control: RateControlMode::Bitrate(self.config.bitrate),
            }),
        ).await?;

        self.control_tx = Some(control_tx);
        self.event_rx = Some(event_rx);

        tracing::info!(
            width = self.config.width,
            height = self.config.height,
            frame_rate = self.config.frame_rate,
            bitrate = self.config.bitrate,
            "OpenH264 encoder started"
        );
        Ok(())
    }

    async fn on_stop(&mut self) -> Result<()> {
        self.control_tx = None;
        self.event_rx = None;
        Ok(())
    }

    async fn on_reset(&mut self) -> Result<()> {
        self.control_tx = None;
        self.event_rx = None;
        Ok(())
    }

    async fn process(&mut self, input: Self::Input) -> Result<Option<Self::Output>> {
        let control_tx = self.control_tx.as_ref()
            .ok_or_else(|| anyhow!("Encoder not initialized"))?;

        control_tx.send(encoder::EncoderControl::Frame(
            input.texture,
            input.timestamp,
            input.statistics,
        )).await?;

        let event_rx = self.event_rx.as_mut()
            .ok_or_else(|| anyhow!("Encoder not initialized"))?;

        match event_rx.try_recv() {
            Ok(encoder::EncoderEvent::Data(buffer)) => {
                Ok(Some(EncodedData { buffer }))
            }
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                Err(anyhow!("Encoder channel disconnected"))
            }
        }
    }
}

impl Encoder for OpenH264Encoder {
    fn encoding(&self) -> Encoding {
        Encoding::H264
    }

    fn set_bitrate(&mut self, _bitrate: u32) {
    }

    fn request_keyframe(&mut self) {
    }
}

