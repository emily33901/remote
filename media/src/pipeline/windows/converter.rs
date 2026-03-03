use std::sync::Arc;
use std::mem::ManuallyDrop;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use windows::{
    core::Interface,
    Win32::{
        Foundation::RECT,
        Graphics::Direct3D11::{
            ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
            ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
            ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
            D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CAPS,
            D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
            D3D11_VIDEO_PROCESSOR_OUTPUT_RATE_NORMAL, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
            D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
        },
    },
};

use crate::lifecycle::{Stage, StageMeta, AtomicLifecycleState, LifecycleState};
use crate::texture_pool::TexturePool;
use crate::dx::{self, ID3D11Texture2DExt};

use crate::pipeline::traits::Converter;
use crate::pipeline::types::{ConvertFrame, PixelFormat};

#[derive(Debug, Clone, Copy)]
pub enum DxvaFormat {
    BGRA,
    NV12,
}

impl From<DxvaFormat> for dx::TextureFormat {
    fn from(value: DxvaFormat) -> Self {
        match value {
            DxvaFormat::BGRA => dx::TextureFormat::BGRA,
            DxvaFormat::NV12 => dx::TextureFormat::NV12,
        }
    }
}

impl From<PixelFormat> for DxvaFormat {
    fn from(value: PixelFormat) -> Self {
        match value {
            PixelFormat::BGRA => DxvaFormat::BGRA,
            PixelFormat::NV12 => DxvaFormat::NV12,
            PixelFormat::I420 => DxvaFormat::NV12,
            PixelFormat::RGB24 => DxvaFormat::BGRA,
        }
    }
}

impl From<DxvaFormat> for PixelFormat {
    fn from(value: DxvaFormat) -> Self {
        match value {
            DxvaFormat::BGRA => PixelFormat::BGRA,
            DxvaFormat::NV12 => PixelFormat::NV12,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DxvaConverterConfig {
    pub input_format: PixelFormat,
    pub output_format: PixelFormat,
    pub output_width: u32,
    pub output_height: u32,
}

struct VideoProcessor {
    processor: ID3D11VideoProcessor,
    video_context: ID3D11VideoContext,
    input_texture: ID3D11Texture2D,
    output_texture: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    input_view: ID3D11VideoProcessorInputView,
    input_width: u32,
    input_height: u32,
}

impl VideoProcessor {
    unsafe fn create(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        input_width: u32,
        input_height: u32,
        output_width: u32,
        output_height: u32,
        input_format: DxvaFormat,
        output_format: DxvaFormat,
    ) -> Result<Self> {
        let video_device: ID3D11VideoDevice = device.cast()?;

        let content_description = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputWidth: input_width,
            InputHeight: input_height,
            OutputWidth: output_width,
            OutputHeight: output_height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            ..Default::default()
        };

        let enumerator = video_device.CreateVideoProcessorEnumerator(&content_description)?;

        let mut caps = D3D11_VIDEO_PROCESSOR_CAPS::default();
        enumerator.GetVideoProcessorCaps(&mut caps)?;

        let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;
        let video_context: ID3D11VideoContext = context.cast()?;

        video_context.VideoProcessorSetStreamFrameFormat(
            &processor,
            0,
            D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        );

        video_context.VideoProcessorSetStreamOutputRate(
            &processor,
            0,
            D3D11_VIDEO_PROCESSOR_OUTPUT_RATE_NORMAL,
            true,
            None,
        );

        let input_rect = RECT {
            left: 0, top: 0,
            right: input_width as i32,
            bottom: input_height as i32,
        };

        let output_rect = RECT {
            left: 0, top: 0,
            right: output_width as i32,
            bottom: output_height as i32,
        };

        video_context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&input_rect));
        video_context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&output_rect));
        video_context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&output_rect));

        let input_texture = dx::TextureBuilder::new(
            device, input_width, input_height, input_format.into(),
        ).build()?;

        let mut input_view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC::default();
        input_view_desc.FourCC = 0;
        input_view_desc.ViewDimension = D3D11_VPIV_DIMENSION_TEXTURE2D;

        let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
        video_device.CreateVideoProcessorInputView(
            &input_texture, &enumerator, &input_view_desc, Some(&mut input_view),
        )?;

        let output_texture = dx::TextureBuilder::new(
            device, output_width, output_height, output_format.into(),
        ).bind_render_target().build()?;

        let mut output_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC::default();
        output_view_desc.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;

        let mut output_view: Option<ID3D11VideoProcessorOutputView> = None;
        video_device.CreateVideoProcessorOutputView(
            &output_texture, &enumerator, &output_view_desc, Some(&mut output_view),
        )?;

        Ok(Self {
            processor,
            video_context,
            input_texture,
            output_texture,
            output_view: output_view.unwrap(),
            input_view: input_view.unwrap(),
            input_width,
            input_height,
        })
    }

    fn process(&self, input: &ID3D11Texture2D, output: &ID3D11Texture2D) -> Result<()> {
        dx::copy_texture(&self.input_texture, input, None)?;

        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(self.input_view.clone())),
            ..Default::default()
        };

        unsafe {
            self.video_context.VideoProcessorBlt(
                &self.processor,
                Some(&self.output_view),
                0,
                &[stream],
            )?;
        }

        dx::copy_texture(output, &self.output_texture, None)?;

        Ok(())
    }
}

pub struct DxvaConverter {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: DxvaConverterConfig,
    device: Option<ID3D11Device>,
    context: Option<ID3D11DeviceContext>,
    processor: Option<VideoProcessor>,
    texture_pool: Option<TexturePool>,
}

impl DxvaConverter {
    pub fn new(config: DxvaConverterConfig) -> Self {
        Self {
            meta: StageMeta::new("dxva-converter"),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            config,
            device: None,
            context: None,
            processor: None,
            texture_pool: None,
        }
    }

    fn maybe_rebuild_processor(&mut self, input_width: u32, input_height: u32) -> Result<()> {
        let needs_rebuild = self.processor.as_ref()
            .map(|p| p.input_width != input_width || p.input_height != input_height)
            .unwrap_or(true);

        if needs_rebuild {
            let device = self.device.as_ref()
                .ok_or_else(|| anyhow!("Device not initialized"))?;
            let context = self.context.as_ref()
                .ok_or_else(|| anyhow!("Context not initialized"))?;

            self.processor = Some(unsafe {
                VideoProcessor::create(
                    device, context,
                    input_width, input_height,
                    self.config.output_width, self.config.output_height,
                    self.config.input_format.into(),
                    self.config.output_format.into(),
                )?
            });

            tracing::debug!(
                input_width, input_height,
                output_width = self.config.output_width,
                output_height = self.config.output_height,
                "Rebuilt video processor"
            );
        }

        Ok(())
    }
}

#[async_trait]
impl Stage for DxvaConverter {
    type Input = ConvertFrame;
    type Output = ConvertFrame;

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
        let (device, context) = dx::create_device()?;
        self.device = Some(device.clone());
        self.context = Some(context);

        let texture_pool = TexturePool::new(
            || {
                dx::TextureBuilder::new(
                    &device,
                    self.config.output_width,
                    self.config.output_height,
                    DxvaFormat::from(self.config.output_format).into(),
                )
                .nt_handle()
                .keyed_mutex()
                .build()
                .unwrap()
            },
            10,
        );
        self.texture_pool = Some(texture_pool);

        tracing::info!(
            input_format = ?self.config.input_format,
            output_format = ?self.config.output_format,
            output_width = self.config.output_width,
            output_height = self.config.output_height,
            "DxvaConverter started"
        );

        Ok(())
    }

    async fn on_stop(&mut self) -> Result<()> {
        self.processor = None;
        self.texture_pool = None;
        self.context = None;
        self.device = None;
        Ok(())
    }

    async fn on_reset(&mut self) -> Result<()> {
        self.processor = None;
        Ok(())
    }

    async fn process(&mut self, input: Self::Input) -> Result<Option<Self::Output>> {
        let desc = input.texture.desc();
        self.maybe_rebuild_processor(desc.Width, desc.Height)?;

        let processor = self.processor.as_ref()
            .ok_or_else(|| anyhow!("Video processor not initialized"))?;
        let texture_pool = self.texture_pool.as_ref()
            .ok_or_else(|| anyhow!("Texture pool not initialized"))?;

        let output = texture_pool.acquire();
        processor.process(&input.texture, &output)?;

        Ok(Some(ConvertFrame {
            texture: output,
            timestamp: input.timestamp,
            format: self.config.output_format,
        }))
    }
}

impl Converter for DxvaConverter {
    fn input_format(&self) -> PixelFormat {
        self.config.input_format
    }

    fn output_format(&self) -> PixelFormat {
        self.config.output_format
    }
}
