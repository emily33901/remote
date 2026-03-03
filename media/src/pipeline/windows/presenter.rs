use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use windows::Win32::{
    Foundation::HWND,
    Graphics::{
        Direct3D11::{
            ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
            D3D11_TEXTURE2D_DESC,
        },
        Dxgi::{IDXGISwapChain, DXGI_PRESENT, DXGI_SWAP_CHAIN_FLAG},
    },
};

use crate::dx;
use crate::lifecycle::{Stage, StageMeta, AtomicLifecycleState, LifecycleState};
use crate::pipeline::traits::Presenter;
use crate::pipeline::types::PresentFrame;

#[derive(Debug, Clone)]
pub struct D3D11PresenterConfig {
    pub hwnd: isize,
    pub width: u32,
    pub height: u32,
}

pub struct D3D11Presenter {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: D3D11PresenterConfig,
    device: Option<ID3D11Device>,
    context: Option<ID3D11DeviceContext>,
    swapchain: Option<IDXGISwapChain>,
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
            device: None,
            context: None,
            swapchain: None,
            current_width: width,
            current_height: height,
        }
    }

    fn resize_swapchain(&mut self, width: u32, height: u32) -> Result<()> {
        let Some(swapchain) = &self.swapchain else {
            return Err(anyhow!("Swapchain not initialized"));
        };

        unsafe {
            swapchain.ResizeBuffers(
                0,
                width,
                height,
                windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }

        self.current_width = width;
        self.current_height = height;
        tracing::info!(width, height, "Swapchain resized");
        Ok(())
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
        let hwnd = HWND(self.config.hwnd as *mut std::ffi::c_void);
        let (device, context, swapchain) = 
            dx::create_device_and_swapchain(hwnd, self.config.width, self.config.height)?;

        self.device = Some(device);
        self.context = Some(context);
        self.swapchain = Some(swapchain);

        tracing::info!(
            width = self.config.width,
            height = self.config.height,
            "D3D11 presenter started"
        );
        Ok(())
    }

    async fn on_stop(&mut self) -> Result<()> {
        self.swapchain = None;
        self.context = None;
        self.device = None;
        Ok(())
    }

    async fn on_reset(&mut self) -> Result<()> {
        self.swapchain = None;
        self.context = None;
        self.device = None;
        Ok(())
    }

    async fn process(&mut self, input: Self::Input) -> Result<Option<Self::Output>> {
        let src_texture: ID3D11Texture2D = (*input.texture).clone();

        let mut src_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { src_texture.GetDesc(&mut src_desc); }

        if src_desc.Width != self.current_width || src_desc.Height != self.current_height {
            self.resize_swapchain(src_desc.Width, src_desc.Height)?;
        }

        let swapchain = self.swapchain.as_ref()
            .ok_or_else(|| anyhow!("Swapchain not initialized"))?;
        let context = self.context.as_ref()
            .ok_or_else(|| anyhow!("Context not initialized"))?;

        let backbuffer: ID3D11Texture2D = unsafe {
            swapchain.GetBuffer(0)?
        };

        unsafe {
            context.CopyResource(&backbuffer, &src_texture);
            let _ = swapchain.Present(1, DXGI_PRESENT::default());
        }

        Ok(None)
    }
}

impl Presenter for D3D11Presenter {
    fn dimensions(&self) -> (u32, u32) {
        (self.current_width, self.current_height)
    }

    fn resize(&mut self, width: u32, height: u32) {
        if let Err(e) = self.resize_swapchain(width, height) {
            tracing::error!(error = %e, "Failed to resize swapchain");
        }
    }
}
