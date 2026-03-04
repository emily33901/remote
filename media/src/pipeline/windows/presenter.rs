use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use windows::{
    core::s,
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Dxgi::{
                Common::{DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R8G8_UNORM, DXGI_FORMAT_R8_UNORM},
                IDXGISwapChain, DXGI_PRESENT, DXGI_SWAP_CHAIN_FLAG,
            },
        },
    },
};

use crate::dx::{self, compile_shader, ID3D11Texture2DExt};
use crate::lifecycle::{AtomicLifecycleState, LifecycleState, Stage, StageMeta};
use crate::pipeline::traits::Presenter;
use crate::pipeline::types::PresentFrame;

#[derive(Debug, Clone)]
pub struct D3D11PresenterConfig {
    pub hwnd: isize,
    pub width: u32,
    pub height: u32,
}

struct NV12TextureHolder {
    width: u32,
    height: u32,
    texture: ID3D11Texture2D,
    chrom_view: ID3D11ShaderResourceView,
    lum_view: ID3D11ShaderResourceView,
}

impl NV12TextureHolder {
    fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<Self> {
        let texture = dx::TextureBuilder::new(device, width, height, dx::TextureFormat::NV12)
            .bind_shader_resource()
            .build()?;

        let lum_view = unsafe {
            let mut view: Option<ID3D11ShaderResourceView> = None;
            let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R8_UNORM,
                ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                    },
                },
            };
            device.CreateShaderResourceView(&texture, Some(&desc), Some(&mut view))?;
            view.unwrap()
        };

        let chrom_view = unsafe {
            let mut view: Option<ID3D11ShaderResourceView> = None;
            let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R8G8_UNORM,
                ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                    },
                },
            };
            device.CreateShaderResourceView(&texture, Some(&desc), Some(&mut view))?;
            view.unwrap()
        };

        Ok(Self {
            width,
            height,
            texture,
            chrom_view,
            lum_view,
        })
    }
}

#[repr(C)]
struct Vertex {
    x: f32,
    y: f32,
    u: f32,
    v: f32,
}

struct NV12Renderer {
    sampler: ID3D11SamplerState,
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    vertex_buffer: ID3D11Buffer,
    input_layout: ID3D11InputLayout,
    texture_holder: Mutex<Option<NV12TextureHolder>>,
}

impl NV12Renderer {
    fn new(device: &ID3D11Device) -> Result<Self> {
        let sampler = unsafe {
            let desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MinLOD: 0.0,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut sampler: Option<ID3D11SamplerState> = None;
            device.CreateSamplerState(&desc, Some(&mut sampler))?;
            sampler.unwrap()
        };

        let vs_blob = compile_shader(include_str!("shader.hlsl"), s!("vs_main"), s!("vs_5_0"))?;
        let vertex_shader = unsafe {
            let vs_bytes = std::slice::from_raw_parts(
                vs_blob.GetBufferPointer() as *const u8,
                vs_blob.GetBufferSize(),
            );
            let mut shader: Option<ID3D11VertexShader> = None;
            device.CreateVertexShader(vs_bytes, None, Some(&mut shader))?;
            shader.unwrap()
        };

        let ps_blob = compile_shader(include_str!("shader.hlsl"), s!("ps_main"), s!("ps_5_0"))?;
        let pixel_shader = unsafe {
            let ps_bytes = std::slice::from_raw_parts(
                ps_blob.GetBufferPointer() as *const u8,
                ps_blob.GetBufferSize(),
            );
            let mut shader: Option<ID3D11PixelShader> = None;
            device.CreatePixelShader(ps_bytes, None, Some(&mut shader))?;
            shader.unwrap()
        };

        let input_layout = unsafe {
            let vs_bytes = std::slice::from_raw_parts(
                vs_blob.GetBufferPointer() as *const u8,
                vs_blob.GetBufferSize(),
            );
            let elements = &[
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("POS"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 0,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("TEX"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: D3D11_APPEND_ALIGNED_ELEMENT,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
            ];
            let mut layout: Option<ID3D11InputLayout> = None;
            device.CreateInputLayout(elements, vs_bytes, Some(&mut layout))?;
            layout.unwrap()
        };

        let vertices: [Vertex; 6] = [
            Vertex { x: -1.0, y: -1.0, u: 0.0, v: 1.0 },
            Vertex { x: -1.0, y: 1.0, u: 0.0, v: 0.0 },
            Vertex { x: 1.0, y: 1.0, u: 1.0, v: 0.0 },
            Vertex { x: 1.0, y: -1.0, u: 1.0, v: 1.0 },
            Vertex { x: -1.0, y: -1.0, u: 0.0, v: 1.0 },
            Vertex { x: 1.0, y: 1.0, u: 1.0, v: 0.0 },
        ];

        let vertex_buffer = unsafe {
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: (vertices.len() * std::mem::size_of::<Vertex>()) as u32,
                BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
                ..Default::default()
            };
            let data = D3D11_SUBRESOURCE_DATA {
                pSysMem: vertices.as_ptr() as *const _,
                ..Default::default()
            };
            let mut buffer: Option<ID3D11Buffer> = None;
            device.CreateBuffer(&desc, Some(&data), Some(&mut buffer))?;
            buffer.unwrap()
        };

        Ok(Self {
            sampler,
            vertex_shader,
            pixel_shader,
            vertex_buffer,
            input_layout,
            texture_holder: Mutex::new(None),
        })
    }

    fn render(&self, src_texture: &ID3D11Texture2D, device: &ID3D11Device) -> Result<()> {
        let context = unsafe { device.GetImmediateContext()? };
        let desc = src_texture.desc();

        {
            let mut holder = self.texture_holder.lock();
            let needs_rebuild = holder
                .as_ref()
                .map(|h| h.width != desc.Width || h.height != desc.Height)
                .unwrap_or(true);

            if needs_rebuild {
                *holder = Some(NV12TextureHolder::new(device, desc.Width, desc.Height)?);
            }
        }

        let holder = self.texture_holder.lock();
        let holder = holder.as_ref().ok_or_else(|| anyhow!("Texture holder not initialized"))?;

        dx::copy_texture(&holder.texture, src_texture, None)?;

        unsafe {
            context.IASetInputLayout(&self.input_layout);
            context.VSSetShader(&self.vertex_shader, None);
            context.PSSetShader(&self.pixel_shader, None);
            context.PSSetShaderResources(
                0,
                Some(&[Some(holder.lum_view.clone()), Some(holder.chrom_view.clone())]),
            );
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));

            let stride = std::mem::size_of::<Vertex>() as u32;
            let offset = 0u32;
            context.IASetVertexBuffers(
                0,
                1,
                Some(&Some(self.vertex_buffer.clone())),
                Some(&stride),
                Some(&mut offset.clone()),
            );
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.Draw(6, 0);
        }

        Ok(())
    }
}

pub struct D3D11Presenter {
    meta: StageMeta,
    state: Arc<AtomicLifecycleState>,
    config: D3D11PresenterConfig,
    device: Option<ID3D11Device>,
    context: Option<ID3D11DeviceContext>,
    swapchain: Option<IDXGISwapChain>,
    render_target: Option<ID3D11RenderTargetView>,
    renderer: Option<NV12Renderer>,
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
            render_target: None,
            renderer: None,
            current_width: width,
            current_height: height,
        }
    }

    fn resize_swapchain(&mut self, width: u32, height: u32) -> Result<()> {
        let Some(swapchain) = &self.swapchain else {
            return Err(anyhow!("Swapchain not initialized"));
        };
        let Some(device) = &self.device else {
            return Err(anyhow!("Device not initialized"));
        };

        self.render_target = None;

        unsafe {
            swapchain.ResizeBuffers(
                0,
                width,
                height,
                windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }

        let backbuffer: ID3D11Texture2D = unsafe { swapchain.GetBuffer(0)? };
        let render_target = unsafe {
            let mut rtv: Option<ID3D11RenderTargetView> = None;
            device.CreateRenderTargetView(&backbuffer, None, Some(&mut rtv))?;
            rtv.unwrap()
        };

        self.render_target = Some(render_target);
        self.current_width = width;
        self.current_height = height;

        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: width as f32,
            Height: height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };

        if let Some(context) = &self.context {
            unsafe { context.RSSetViewports(Some(&[viewport])) };
        }

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

        let backbuffer: ID3D11Texture2D = unsafe { swapchain.GetBuffer(0)? };
        let render_target = unsafe {
            let mut rtv: Option<ID3D11RenderTargetView> = None;
            device.CreateRenderTargetView(&backbuffer, None, Some(&mut rtv))?;
            rtv.unwrap()
        };

        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: self.config.width as f32,
            Height: self.config.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        unsafe { context.RSSetViewports(Some(&[viewport])) };

        let renderer = NV12Renderer::new(&device)?;

        self.device = Some(device);
        self.context = Some(context);
        self.swapchain = Some(swapchain);
        self.render_target = Some(render_target);
        self.renderer = Some(renderer);

        tracing::info!(
            width = self.config.width,
            height = self.config.height,
            "D3D11 presenter started"
        );
        Ok(())
    }

    async fn on_stop(&mut self) -> Result<()> {
        self.renderer = None;
        self.render_target = None;
        self.swapchain = None;
        self.context = None;
        self.device = None;
        Ok(())
    }

    async fn on_reset(&mut self) -> Result<()> {
        self.renderer = None;
        self.render_target = None;
        self.swapchain = None;
        self.context = None;
        self.device = None;
        Ok(())
    }

    async fn process(&mut self, input: Self::Input) -> Result<Option<Self::Output>> {
        let src_texture: ID3D11Texture2D = (*input.texture).clone();

        let mut src_desc = Default::default();
        unsafe { src_texture.GetDesc(&mut src_desc); }

        if src_desc.Width != self.current_width || src_desc.Height != self.current_height {
            self.resize_swapchain(src_desc.Width, src_desc.Height)?;
        }

        let swapchain = self.swapchain.as_ref()
            .ok_or_else(|| anyhow!("Swapchain not initialized"))?;
        let context = self.context.as_ref()
            .ok_or_else(|| anyhow!("Context not initialized"))?;
        let render_target = self.render_target.as_ref()
            .ok_or_else(|| anyhow!("Render target not initialized"))?;
        let renderer = self.renderer.as_ref()
            .ok_or_else(|| anyhow!("Renderer not initialized"))?;
        let device = self.device.as_ref()
            .ok_or_else(|| anyhow!("Device not initialized"))?;

        unsafe {
            context.OMSetRenderTargets(Some(&[Some(render_target.clone())]), None);
            context.ClearRenderTargetView(render_target, &[0.0, 0.0, 0.0, 1.0]);
        }

        renderer.render(&src_texture, device)?;

        unsafe {
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
