use std::sync::Arc;

use eyre::Result;
use tokio::sync::{mpsc, mpsc::error::TryRecvError};

use windows::{
    core::{s, PCSTR},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, S_OK, WPARAM},
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Dxgi::{
                Common::{
                    DXGI_FORMAT, DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R8G8_UNORM,
                    DXGI_FORMAT_R8_UNORM,
                },
                IDXGISwapChain,
            },
        },
        System::LibraryLoader::GetModuleHandleA,
        UI::WindowsAndMessaging::{
            CreateWindowExA, DefWindowProcA, DispatchMessageA, LoadCursorW, PeekMessageA,
            PostQuitMessage, RegisterClassA, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
            CW_USEDEFAULT, IDC_ARROW, MSG, PM_REMOVE, WINDOW_EX_STYLE, WM_CREATE, WM_DESTROY,
            WNDCLASSA, WS_MINIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_SYSMENU, WS_VISIBLE,
        },
    },
};

use winit::{
    dpi::PhysicalSize,
    event_loop::EventLoopBuilder,
    platform::windows::EventLoopBuilderExtWindows,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::WindowBuilder,
};

use crate::ARBITRARY_CHANNEL_LIMIT;

use media::dx::{
    self, compile_shader, copy_texture, create_device_and_swapchain, ID3D11Texture2DExt,
};

fn create_render_target_for_swap_chain(
    device: &ID3D11Device,
    swap_chain: &IDXGISwapChain,
) -> Result<ID3D11RenderTargetView> {
    let swap_chain_texture = unsafe { swap_chain.GetBuffer::<ID3D11Texture2D>(0) }?;
    let mut render_target = None;
    unsafe { device.CreateRenderTargetView(&swap_chain_texture, None, Some(&mut render_target)) }?;
    Ok(render_target.unwrap())
}

fn resize_swap_chain_and_render_target(
    device: &ID3D11Device,
    swap_chain: &IDXGISwapChain,
    render_target: &mut Option<ID3D11RenderTargetView>,
    new_width: u32,
    new_height: u32,
    new_format: DXGI_FORMAT,
) -> Result<()> {
    render_target.take();

    unsafe { swap_chain.ResizeBuffers(1, new_width, new_height, new_format, 0) }?;
    render_target.replace(create_render_target_for_swap_chain(device, swap_chain)?);
    Ok(())
}

struct NV12TextureHolder {
    width: u32,
    height: u32,
    texture: ID3D11Texture2D,
    texture_chrom_view: ID3D11ShaderResourceView,
    texture_lum_view: ID3D11ShaderResourceView,
}

impl NV12TextureHolder {
    fn new(
        device: &ID3D11Device,
        _context: &ID3D11DeviceContext,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let texture = dx::TextureBuilder::new(&device, width, height, dx::TextureFormat::NV12)
            .bind_shader_resource()
            .build()?;

        let mut texture_lum_view: Option<ID3D11ShaderResourceView> = None;

        unsafe {
            let mut texture_view_desc: D3D11_SHADER_RESOURCE_VIEW_DESC =
                D3D11_SHADER_RESOURCE_VIEW_DESC {
                    Format: DXGI_FORMAT_R8_UNORM,
                    ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                };

            texture_view_desc.Anonymous.Texture2D = D3D11_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
            };

            device.CreateShaderResourceView(
                &texture,
                Some(&texture_view_desc as *const _),
                Some(&mut texture_lum_view as *mut _),
            )
        }?;

        let mut texture_chrom_view: Option<ID3D11ShaderResourceView> = None;

        unsafe {
            let mut texture_view_desc: D3D11_SHADER_RESOURCE_VIEW_DESC =
                D3D11_SHADER_RESOURCE_VIEW_DESC {
                    Format: DXGI_FORMAT_R8G8_UNORM,
                    ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                };

            texture_view_desc.Anonymous.Texture2D = D3D11_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
            };

            device.CreateShaderResourceView(
                &texture,
                Some(&texture_view_desc),
                Some(&mut texture_chrom_view),
            )
        }?;

        Ok(Self {
            width,
            height,
            texture: texture,
            texture_chrom_view: texture_chrom_view.unwrap(),
            texture_lum_view: texture_lum_view.unwrap(),
        })
    }
}

pub(crate) struct NV12TextureRender {
    sampler_state: ID3D11SamplerState,
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    vertex_buffer: ID3D11Buffer,
    texture_holder: parking_lot::Mutex<Option<NV12TextureHolder>>,
    input_layout: ID3D11InputLayout,
}

#[repr(C)]
struct Vertex {
    x: f32,
    y: f32,
    u: f32,
    v: f32,
}

impl NV12TextureRender {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let context = unsafe { device.GetImmediateContext()? };

        let sample_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            MinLOD: 0.0,
            MaxLOD: f32::MAX,
            ..Default::default()
        };

        let mut sampler_state: Option<ID3D11SamplerState> = None;
        unsafe { device.CreateSamplerState(&sample_desc, Some(&mut sampler_state as *mut _)) }?;

        unsafe { context.PSSetSamplers(0, Some(&[sampler_state.clone()])) };

        let vertex_shader_blob =
            compile_shader(include_str!("shader.hlsl"), s!("vs_main"), s!("vs_5_0"))?;
        let mut vertex_shader: Option<ID3D11VertexShader> = None;
        unsafe {
            let vertex_shader_blob_buffer = std::slice::from_raw_parts(
                vertex_shader_blob.GetBufferPointer() as *const u8,
                vertex_shader_blob.GetBufferSize(),
            );
            device.CreateVertexShader(
                vertex_shader_blob_buffer,
                None,
                Some(&mut vertex_shader as *mut _),
            )
        }?;

        let vertex_shader = vertex_shader.unwrap();

        let pixel_shader_blob =
            compile_shader(include_str!("shader.hlsl"), s!("ps_main"), s!("ps_5_0"))?;
        let mut pixel_shader: Option<ID3D11PixelShader> = None;
        unsafe {
            let pixel_shader_blob_buffer = std::slice::from_raw_parts(
                pixel_shader_blob.GetBufferPointer() as *const u8,
                pixel_shader_blob.GetBufferSize(),
            );
            device.CreatePixelShader(
                pixel_shader_blob_buffer,
                None,
                Some(&mut pixel_shader as *mut _),
            )
        }?;

        let pixel_shader = pixel_shader.unwrap();

        let pipeline_items = &[
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

        let mut input_layout: Option<ID3D11InputLayout> = None;
        unsafe {
            let vertex_shader_blob_buffer = std::slice::from_raw_parts(
                vertex_shader_blob.GetBufferPointer() as *const u8,
                vertex_shader_blob.GetBufferSize(),
            );
            device.CreateInputLayout(
                pipeline_items,
                vertex_shader_blob_buffer,
                Some(&mut input_layout as *mut _),
            )
        }?;

        let input_layout = input_layout.unwrap();

        let verticies = &[
            Vertex {
                x: -1.0,
                y: 1.0,
                u: 0.0,
                v: 0.0,
            },
            Vertex {
                x: 1.0,
                y: -1.0,
                u: 1.0,
                v: 1.0,
            },
            Vertex {
                x: -1.0,
                y: -1.0,
                u: 0.0,
                v: 1.0,
            },
            Vertex {
                x: -1.0,
                y: 1.0,
                u: 0.0,
                v: 0.0,
            },
            Vertex {
                x: 1.0,
                y: 1.0,
                u: 1.0,
                v: 0.0,
            },
            Vertex {
                x: 1.0,
                y: -1.0,
                u: 1.0,
                v: 1.0,
            },
        ];

        let _topology = D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;

        let buffer_desc = D3D11_BUFFER_DESC {
            Usage: D3D11_USAGE_DEFAULT,
            ByteWidth: (verticies.len() * std::mem::size_of::<Vertex>()) as u32,
            BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
            ..Default::default()
        };

        let subresource_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: verticies.as_ptr() as *const std::ffi::c_void,
            ..Default::default()
        };

        let mut vertex_buffer: Option<ID3D11Buffer> = None;

        unsafe {
            device.CreateBuffer(
                &buffer_desc as *const _,
                Some(&subresource_data as *const _),
                Some(&mut vertex_buffer as *mut _),
            )
        }?;

        Ok(Self {
            sampler_state: sampler_state.unwrap(),
            vertex_shader,
            pixel_shader,
            vertex_buffer: vertex_buffer.unwrap(),
            input_layout: input_layout,
            texture_holder: Default::default(),
        })
    }

    pub(crate) fn render_texture(
        &self,
        texture: &ID3D11Texture2D,
        device: &ID3D11Device,
    ) -> Result<()> {
        let context = unsafe { device.GetImmediateContext()? };

        // NOTE(emily): Always expect to be able to lock here, there should NEVER be any contention over this.

        // NOTE(emily): Unwrap or default because we dont care whether we already have a holder or not
        // if the sizes DONT match then we make a new holder.
        let mut texture_holder = self
            .texture_holder
            .try_lock()
            .expect("there should be no contention on the texture_holder");
        {
            let (width, height) = texture_holder
                .as_ref()
                .map(|h| (h.width, h.height))
                .unwrap_or_default();
            let (input_width, input_height) = {
                let desc = texture.desc();
                (desc.Width, desc.Height)
            };

            if width != input_width || height != input_height {
                *texture_holder = Some(NV12TextureHolder::new(
                    device,
                    &context,
                    input_width,
                    input_height,
                )?);
            }
        }

        let texture_holder = texture_holder.as_ref().unwrap();

        dx::copy_texture(&texture_holder.texture, texture, None)?;

        let topology = D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;

        unsafe {
            context.IASetInputLayout(&self.input_layout);
            context.VSSetShader(&self.vertex_shader, None);
            context.PSSetShader(&self.pixel_shader, None);
        }

        unsafe {
            context.PSSetShaderResources(
                0,
                Some(&[
                    Some(texture_holder.texture_lum_view.clone()),
                    Some(texture_holder.texture_chrom_view.clone()),
                ]),
            );
            context.PSSetSamplers(0, Some(&[Some(self.sampler_state.clone())]));
        }

        unsafe {
            let mut offset = 0_u32;
            let stride = std::mem::size_of::<Vertex>() as u32;
            context.IASetVertexBuffers(
                0,
                1,
                Some(&Some(self.vertex_buffer.clone()) as *const _),
                Some(&stride as *const _),
                Some(&mut offset),
            );
            context.IASetPrimitiveTopology(topology);
        }

        const VERTICES: &[Vertex; 6] = &[
            Vertex {
                x: -1.0,
                y: 1.0,
                u: 0.0,
                v: 0.0,
            },
            Vertex {
                x: 1.0,
                y: -1.0,
                u: 1.0,
                v: 1.0,
            },
            Vertex {
                x: -1.0,
                y: -1.0,
                u: 0.0,
                v: 1.0,
            },
            Vertex {
                x: -1.0,
                y: 1.0,
                u: 0.0,
                v: 0.0,
            },
            Vertex {
                x: 1.0,
                y: 1.0,
                u: 1.0,
                v: 0.0,
            },
            Vertex {
                x: 1.0,
                y: -1.0,
                u: 1.0,
                v: 1.0,
            },
        ];

        unsafe { context.Draw(VERTICES.len() as u32, 0) };

        Ok(())
    }
}

extern "system" fn wndproc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_CREATE => {
                println!("wm_create");
                LRESULT(0)
            }

            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }

            _ => DefWindowProcA(window, message, wparam, lparam),
        }
    }
}

fn make_window(parent: Option<HWND>, window_class: PCSTR) -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleA(None)?;
        debug_assert!(instance.0 != 0);

        let wc = WNDCLASSA {
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hInstance: instance.into(),
            lpszClassName: window_class,

            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            ..Default::default()
        };

        let atom = RegisterClassA(&wc);
        debug_assert!(atom != 0);

        let window_handle = CreateWindowExA(
            WINDOW_EX_STYLE::default(),
            window_class,
            window_class,
            WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1920,
            1080,
            parent.unwrap_or_default(),
            None,
            instance,
            None,
        );

        Ok(window_handle)
    }
}

pub(crate) fn sink(
    width: u32,
    height: u32,
    name: &str,
) -> Result<mpsc::Sender<(Arc<media::Texture>, media::Timestamp)>> {
    let (tx, mut rx) =
        mpsc::channel::<(Arc<media::Texture>, media::Timestamp)>(ARBITRARY_CHANNEL_LIMIT);

    tokio::spawn({
        let tx = tx.clone();

        let name = name.to_string();

        async move {
            telemetry::client::watch_channel(&tx, &format!("player-sink-{}", name)).await;

            match tokio::task::spawn_blocking(move || -> eyre::Result<()> {
                let window_title = s!("remote-player-window");
                let window_handle = make_window(None, window_title)?;

                let (device, context, swap_chain) =
                    create_device_and_swapchain(window_handle, width, height)?;

                let render_target =
                    Some(create_render_target_for_swap_chain(&device, &swap_chain)?);

                let render_target = render_target.unwrap();

                let viewport = D3D11_VIEWPORT {
                    TopLeftX: 0.0,
                    TopLeftY: 0.0,
                    Width: width as f32,
                    Height: height as f32,
                    MinDepth: 0.0,
                    MaxDepth: 1.0,
                };

                unsafe { context.RSSetViewports(Some(&[viewport])) };

                // let mut ticks_ticks = None;
                // let mut ticks_time = None;

                let renderer = NV12TextureRender::new(&device)?;

                unsafe { context.OMSetRenderTargets(Some(&[Some(render_target.clone())]), None) };

                loop {
                    unsafe {
                        let mut message = MSG::default();
                        while PeekMessageA(&mut message, None, 0, 0, PM_REMOVE).into() {
                            TranslateMessage(&message);
                            DispatchMessageA(&message);
                        }
                    }

                    if let Some((texture, timestamp)) = match rx.try_recv() {
                        Ok(ok) => Some(ok),
                        Err(TryRecvError::Disconnected) => break,
                        Err(TryRecvError::Empty) => None,
                    } {
                        unsafe {
                            context.ClearRenderTargetView(&render_target, &[0.0, 0.0, 0.2, 1.0])
                        };

                        renderer.render_texture(&texture, &device)?;
                    }

                    unsafe {
                        match swap_chain.Present(1, 0) {
                            S_OK => {}
                            err => {
                                tracing::debug!("Failed present {err}")
                            }
                        }
                    };
                }

                Ok(())
            })
            .await
            .unwrap()
            {
                Ok(_ok) => tracing::warn!("player sink down ok"),
                Err(err) => tracing::error!("player sink down error {err:?}"),
            };
        }
    });

    Ok(tx)
}
