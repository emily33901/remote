use std::sync::Arc;
use std::ops::Deref;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use windows::{
    core::Interface,
    Win32::{
        Graphics::{
            Direct3D11::{
                ID3D11Device, ID3D11Texture2D, D3D11_BOX, D3D11_RESOURCE_MISC_FLAG,
                D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_TEXTURE2D_DESC,
            },
            Dxgi::{
                IDXGIAdapter, IDXGIDevice2, IDXGIKeyedMutex, IDXGIOutput1, IDXGIOutputDuplication,
                IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT,
                DXGI_OUTDUPL_FRAME_INFO,
            },
        },
    },
};

use crate::lifecycle::{Stage, StageMeta, AtomicLifecycleState, LifecycleState};
use crate::texture_pool::{Texture, TexturePool};
use crate::Timestamp;
use crate::dx;

use crate::pipeline::traits::Capture;
use crate::pipeline::types::{CaptureFrame, PixelFormat};

pub struct DesktopDuplicationConfig {
    pub output_index: u32,
}

impl Default for DesktopDuplicationConfig {
    fn default() -> Self {
        Self { output_index: 0 }
    }
}

struct Context {
    width: u32,
    height: u32,
    device: ID3D11Device,
    duplicated: IDXGIOutputDuplication,
    texture_pool: TexturePool,
    start_time: std::time::Instant,
}

impl Context {
    fn create(output_index: u32) -> windows::core::Result<Self> {
        let (device, _context) = dx::create_device()?;
        let dxgi_device: IDXGIDevice2 = device.cast()?;
        let parent: IDXGIAdapter = unsafe { dxgi_device.GetParent() }?;
        let output = unsafe { parent.EnumOutputs(output_index) }?;
        let output: IDXGIOutput1 = output.cast()?;

        let duplicated = unsafe { output.DuplicateOutput(&device) }?;
        let desc = unsafe { duplicated.GetDesc() };
        let (width, height) = (desc.ModeDesc.Width, desc.ModeDesc.Height);

        let texture_pool = TexturePool::new(
            || {
                dx::TextureBuilder::new(&device, width, height, dx::TextureFormat::BGRA)
                    .nt_handle()
                    .keyed_mutex()
                    .build()
                    .unwrap()
            },
            10,
        );

        Ok(Self {
            width,
            height,
            device,
            duplicated,
            texture_pool,
            start_time: std::time::Instant::now(),
        })
    }

    fn acquire_frame(&self) -> Result<Option<CaptureFrame>> {
        unsafe { let _ = self.duplicated.ReleaseFrame(); }

        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut frame_resource: Option<IDXGIResource> = None;

        let result = unsafe {
            self.duplicated.AcquireNextFrame(1000, &mut frame_info, &mut frame_resource)
        };

        match result {
            Ok(()) => {
                if frame_info.AccumulatedFrames == 0 || frame_info.LastPresentTime == 0 {
                    return Ok(None);
                }

                let frame_resource = frame_resource
                    .ok_or_else(|| anyhow!("No frame resource"))?;
                let src_texture: ID3D11Texture2D = frame_resource.cast()?;

                let dst_texture = self.texture_pool.acquire();
                self.copy_texture(&src_texture, &dst_texture)?;

                Ok(Some(CaptureFrame {
                    texture: dst_texture,
                    timestamp: Timestamp::new_diff_instant(self.start_time, std::time::Instant::now()),
                }))
            }
            Err(err) => {
                let code = err.code();
                if code == DXGI_ERROR_WAIT_TIMEOUT {
                    Ok(None)
                } else if code == DXGI_ERROR_ACCESS_LOST {
                    Err(anyhow!("Desktop duplication access lost"))
                } else {
                    Err(anyhow!("Desktop duplication error: {}", err))
                }
            }
        }
    }

    fn copy_texture(&self, src: &ID3D11Texture2D, dst: &ID3D11Texture2D) -> Result<()> {
        let mut src_desc = D3D11_TEXTURE2D_DESC::default();
        let mut dst_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            src.GetDesc(&mut src_desc);
            dst.GetDesc(&mut dst_desc);
        }

        let keyed = if D3D11_RESOURCE_MISC_FLAG(dst_desc.MiscFlags as i32)
            .contains(D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX)
        {
            let keyed: IDXGIKeyedMutex = dst.cast()?;
            unsafe { keyed.AcquireSync(0, u32::MAX)?; }
            Some(keyed)
        } else {
            None
        };

        scopeguard::defer! {
            if let Some(keyed) = keyed {
                unsafe { let _ = keyed.ReleaseSync(0); }
            }
        }

        let device = unsafe { src.GetDevice() }?;
        let context = unsafe { device.GetImmediateContext() }?;

        let region = D3D11_BOX {
            left: 0, top: 0, front: 0,
            right: dst_desc.Width,
            bottom: dst_desc.Height,
            back: 1,
        };

        unsafe {
            context.CopySubresourceRegion(
                dst.deref(), 0, 0, 0, 0,
                src, 0, Some(&region),
            )
        };

        Ok(())
    }
}

pub struct DesktopDuplicationCapture {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: DesktopDuplicationConfig,
    context: Option<Context>,
}

impl DesktopDuplicationCapture {
    pub fn new(config: DesktopDuplicationConfig) -> Self {
        Self {
            meta: StageMeta::new("desktop-duplication"),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            config,
            context: None,
        }
    }

    pub fn dimensions(&self) -> Option<(u32, u32)> {
        self.context.as_ref().map(|c| (c.width, c.height))
    }
}

#[async_trait]
impl Stage for DesktopDuplicationCapture {
    type Input = ();
    type Output = CaptureFrame;

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
        let context = Context::create(self.config.output_index)
            .map_err(|e| anyhow!("Failed to create desktop duplication: {}", e))?;

        tracing::info!(
            width = context.width,
            height = context.height,
            "Desktop duplication started"
        );

        self.context = Some(context);
        Ok(())
    }

    async fn on_stop(&mut self) -> Result<()> {
        self.context = None;
        Ok(())
    }

    async fn on_reset(&mut self) -> Result<()> {
        self.context = None;
        Ok(())
    }

    async fn process(&mut self, _input: Self::Input) -> Result<Option<Self::Output>> {
        let context = self.context.as_ref()
            .ok_or_else(|| anyhow!("Desktop duplication not initialized"))?;

        context.acquire_frame()
    }
}

impl Capture for DesktopDuplicationCapture {
    fn output_format(&self) -> PixelFormat {
        PixelFormat::BGRA
    }
}
