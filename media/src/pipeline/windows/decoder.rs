use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::lifecycle::{Stage, StageMeta, AtomicLifecycleState, LifecycleState};
use crate::{Encoding, decoder};

use crate::pipeline::traits::Decoder;
use crate::pipeline::types::{DecodeFrame, EncodedData};

#[derive(Debug, Clone)]
pub struct MediaFoundationDecoderConfig {
    pub width: u32,
    pub height: u32,
    pub encoding: Encoding,
}

pub struct MediaFoundationDecoder {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: MediaFoundationDecoderConfig,
    control_tx: Option<mpsc::Sender<decoder::DecoderControl>>,
    event_rx: Option<mpsc::Receiver<decoder::DecoderEvent>>,
}

impl MediaFoundationDecoder {
    pub fn new(config: MediaFoundationDecoderConfig) -> Self {
        Self {
            meta: StageMeta::new("media-foundation-decoder"),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            config,
            control_tx: None,
            event_rx: None,
        }
    }
}

#[async_trait]
impl Stage for MediaFoundationDecoder {
    type Input = EncodedData;
    type Output = DecodeFrame;

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
            return Err(anyhow!("Only H264 decoding is currently supported"));
        }

        let dec = decoder::Decoder::MediaFoundation;
        let (control_tx, event_rx) = dec.run(
            self.config.width,
            self.config.height,
            60,
        ).await?;

        self.control_tx = Some(control_tx);
        self.event_rx = Some(event_rx);

        tracing::info!(
            width = self.config.width,
            height = self.config.height,
            encoding = ?self.config.encoding,
            "MediaFoundation decoder started"
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
            .ok_or_else(|| anyhow!("Decoder not initialized"))?;

        control_tx.send(decoder::DecoderControl::Data(input.buffer)).await?;

        let event_rx = self.event_rx.as_mut()
            .ok_or_else(|| anyhow!("Decoder not initialized"))?;

        match event_rx.try_recv() {
            Ok(decoder::DecoderEvent::Frame(texture, timestamp, statistics)) => {
                Ok(Some(DecodeFrame {
                    texture: Arc::new(texture),
                    timestamp,
                    statistics,
                }))
            }
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                Err(anyhow!("Decoder channel disconnected"))
            }
        }
    }
}

impl Decoder for MediaFoundationDecoder {
    fn encoding(&self) -> Encoding {
        self.config.encoding
    }
}

#[derive(Debug, Clone)]
pub struct OpenH264DecoderConfig {
    pub width: u32,
    pub height: u32,
}

pub struct OpenH264Decoder {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: OpenH264DecoderConfig,
    control_tx: Option<mpsc::Sender<decoder::DecoderControl>>,
    event_rx: Option<mpsc::Receiver<decoder::DecoderEvent>>,
}

impl OpenH264Decoder {
    pub fn new(config: OpenH264DecoderConfig) -> Self {
        Self {
            meta: StageMeta::new("openh264-decoder"),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            config,
            control_tx: None,
            event_rx: None,
        }
    }
}

#[async_trait]
impl Stage for OpenH264Decoder {
    type Input = EncodedData;
    type Output = DecodeFrame;

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
        let dec = decoder::Decoder::OpenH264;
        let (control_tx, event_rx) = dec.run(
            self.config.width,
            self.config.height,
            60,
        ).await?;

        self.control_tx = Some(control_tx);
        self.event_rx = Some(event_rx);

        tracing::info!(
            width = self.config.width,
            height = self.config.height,
            "OpenH264 decoder started"
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
            .ok_or_else(|| anyhow!("Decoder not initialized"))?;

        control_tx.send(decoder::DecoderControl::Data(input.buffer)).await?;

        let event_rx = self.event_rx.as_mut()
            .ok_or_else(|| anyhow!("Decoder not initialized"))?;

        match event_rx.try_recv() {
            Ok(decoder::DecoderEvent::Frame(texture, timestamp, statistics)) => {
                Ok(Some(DecodeFrame {
                    texture: Arc::new(texture),
                    timestamp,
                    statistics,
                }))
            }
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                Err(anyhow!("Decoder channel disconnected"))
            }
        }
    }
}

impl Decoder for OpenH264Decoder {
    fn encoding(&self) -> Encoding {
        Encoding::H264
    }
}
