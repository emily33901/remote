use std::mem::MaybeUninit;

use eyre::{eyre, Result};
use windows::{
    core::{ComInterface, PCSTR},
    Win32::{
        Foundation::{HWND, TRUE},
        Graphics::{
            Direct3D::{Fxc::D3DCompile, *},
            Direct3D11::*,
            Dxgi::{
                Common::{
                    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12,
                    DXGI_FORMAT_R8G8B8A8_UNORM,
                },
                IDXGIKeyedMutex, IDXGISwapChain, DXGI_SWAP_CHAIN_DESC,
                DXGI_USAGE_RENDER_TARGET_OUTPUT,
            },
        },
    },
};

const FEATURE_LEVELS: [D3D_FEATURE_LEVEL; 9] = [
    D3D_FEATURE_LEVEL_12_1,
    D3D_FEATURE_LEVEL_12_0,
    D3D_FEATURE_LEVEL_11_1,
    D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_9_3,
    D3D_FEATURE_LEVEL_9_2,
    D3D_FEATURE_LEVEL_9_1,
];

const FLAGS: D3D11_CREATE_DEVICE_FLAG = D3D11_CREATE_DEVICE_FLAG(
    D3D11_CREATE_DEVICE_BGRA_SUPPORT.0 as u32 | D3D11_CREATE_DEVICE_VIDEO_SUPPORT.0 as u32,
);

pub(crate) fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;

        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            FLAGS,
            Some(&FEATURE_LEVELS),
            D3D11_SDK_VERSION,
            Some(&mut device as *mut _),
            None,
            Some(&mut context as *mut _),
        )?;

        let device = device.unwrap();
        let multithreaded_device: ID3D11Multithread = device.cast()?;
        multithreaded_device.SetMultithreadProtected(true);

        Ok((device, context.unwrap()))
    }
}

pub(crate) fn create_device_and_swapchain(
    hwnd: HWND,
    width: u32,
    height: u32,
) -> Result<(ID3D11Device, ID3D11DeviceContext, IDXGISwapChain)> {
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut swap_chain: Option<IDXGISwapChain> = None;

        let swap_chain_desc = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_MODE_DESC {
                Width: width,
                Height: height,
                RefreshRate: windows::Win32::Graphics::Dxgi::Common::DXGI_RATIONAL {
                    Numerator: 60,
                    Denominator: 1,
                },
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                ..Default::default()
            },
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 1,
            OutputWindow: hwnd,
            Windowed: TRUE,
            ..Default::default()
        };

        D3D11CreateDeviceAndSwapChain(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            FLAGS,
            Some(&FEATURE_LEVELS),
            D3D11_SDK_VERSION,
            Some(&swap_chain_desc as *const _),
            Some(&mut swap_chain as *mut _),
            Some(&mut device as *mut _),
            None,
            Some(&mut context as *mut _),
        )?;

        let device = device.unwrap();
        let multithreaded_device: ID3D11Multithread = device.cast()?;
        multithreaded_device.SetMultithreadProtected(true);

        Ok((device, context.unwrap(), swap_chain.unwrap()))
    }
}

pub(crate) enum TextureFormat {
    NV12,
    BGRA,
}

impl From<TextureFormat> for DXGI_FORMAT {
    fn from(value: TextureFormat) -> Self {
        match value {
            TextureFormat::NV12 => DXGI_FORMAT_NV12,
            TextureFormat::BGRA => DXGI_FORMAT_B8G8R8A8_UNORM,
        }
    }
}

// TODO(emily): Staging textures

pub(crate) enum TextureUsage {
    Staging,
}

pub(crate) enum TextureCPUAccess {
    Write,
    Read,
}

pub(crate) struct TextureBuilder<'a> {
    device: &'a ID3D11Device,
    keyed_mutex: bool,
    nt_handle: bool,
    bind_shader_resource: bool,
    bind_render_target: bool,
    bind_unordered_access: bool,
    width: u32,
    height: u32,
    format: TextureFormat,
    usage: Option<TextureUsage>,
    cpu_access: Option<TextureCPUAccess>,
}

impl<'a> TextureBuilder<'a> {
    pub(crate) fn new(
        device: &'a ID3D11Device,
        width: u32,
        height: u32,
        format: TextureFormat,
    ) -> Self {
        Self {
            device,
            bind_shader_resource: false,
            keyed_mutex: false,
            nt_handle: false,
            bind_render_target: false,
            bind_unordered_access: false,
            width,
            height,
            format,
            usage: None,
            cpu_access: None,
        }
    }

    pub(crate) fn bind_render_target(mut self) -> Self {
        self.bind_render_target = true;
        self
    }

    pub(crate) fn keyed_mutex(mut self) -> Self {
        self.keyed_mutex = true;
        self
    }

    pub(crate) fn nt_handle(mut self) -> Self {
        self.nt_handle = true;
        self
    }

    pub(crate) fn bind_shader_resource(mut self) -> Self {
        self.bind_shader_resource = true;
        self
    }

    pub(crate) fn usage(mut self, usage: TextureUsage) -> Self {
        self.usage = Some(usage);
        self
    }

    pub(crate) fn cpu_access(mut self, cpu_access: TextureCPUAccess) -> Self {
        self.cpu_access = Some(cpu_access);
        self
    }

    pub(crate) fn build(self) -> Result<ID3D11Texture2D> {
        let bind_flags =
            0 | if self.bind_shader_resource {
                D3D11_BIND_SHADER_RESOURCE.0 as u32
            } else {
                0
            } | if self.bind_render_target {
                D3D11_BIND_RENDER_TARGET.0 as u32
            } else {
                0
            };

        let misc_flags = if self.keyed_mutex {
            D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32
        } else {
            0
        } | if self.nt_handle {
            D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 as u32
        } else {
            0
        };

        let usage = match self.usage {
            Some(TextureUsage::Staging) => D3D11_USAGE_STAGING,
            None => D3D11_USAGE_DEFAULT,
        };

        let cpu_access_flags = match self.cpu_access {
            Some(TextureCPUAccess::Read) => D3D11_CPU_ACCESS_READ.0 as u32,
            Some(TextureCPUAccess::Write) => D3D11_CPU_ACCESS_WRITE.0 as u32,
            None => 0,
        };

        let description = D3D11_TEXTURE2D_DESC {
            Width: self.width,
            Height: self.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: self.format.into(),
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: usage,
            BindFlags: bind_flags,
            CPUAccessFlags: cpu_access_flags,
            MiscFlags: misc_flags,
        };

        let mut texture: Option<ID3D11Texture2D> = None;

        unsafe {
            self.device.CreateTexture2D(
                &description as *const _,
                None,
                Some(&mut texture as *mut _),
            )?
        };

        texture.ok_or(eyre!("Unable to create texture"))
    }
}

pub(crate) fn copy_texture(
    out_texture: &ID3D11Texture2D,
    in_texture: &ID3D11Texture2D,
    subresource_index: Option<u32>,
) -> windows::core::Result<()> {
    let mut in_desc = D3D11_TEXTURE2D_DESC::default();
    let mut out_desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        in_texture.GetDesc(&mut in_desc as *mut _);
        out_texture.GetDesc(&mut out_desc as *mut _);
    }

    // If keyed mutex then lock keyed muticies

    let keyed_in = if D3D11_RESOURCE_MISC_FLAG(in_desc.MiscFlags as i32)
        .contains(D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX)
    {
        let keyed: IDXGIKeyedMutex = in_texture.cast()?;
        unsafe {
            keyed.AcquireSync(0, u32::MAX)?;
        }
        Some(keyed)
    } else {
        None
    };

    let keyed_out = if D3D11_RESOURCE_MISC_FLAG(out_desc.MiscFlags as i32)
        .contains(D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX)
    {
        let keyed: IDXGIKeyedMutex = out_texture.cast()?;
        unsafe {
            keyed.AcquireSync(0, u32::MAX)?;
        }
        Some(keyed)
    } else {
        None
    };

    let device = unsafe { in_texture.GetDevice() }?;
    let context = unsafe { device.GetImmediateContext() }?;

    let region = D3D11_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: out_desc.Width,
        bottom: out_desc.Height,
        back: 1,
    };

    let subresource_index = subresource_index.unwrap_or_default();

    unsafe {
        context.CopySubresourceRegion(
            out_texture,
            0,
            0,
            0,
            0,
            in_texture,
            subresource_index,
            Some(&region),
        )
    };

    if let Some(keyed) = keyed_out {
        unsafe {
            keyed.ReleaseSync(0)?;
        }
    }

    if let Some(keyed) = keyed_in {
        unsafe {
            keyed.ReleaseSync(0)?;
        }
    }

    Ok(())
}

pub(crate) fn compile_shader(data: &str, entry_point: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    unsafe {
        let mut blob: MaybeUninit<Option<ID3DBlob>> = MaybeUninit::uninit();
        let mut err_blob: Option<ID3DBlob> = None;
        match D3DCompile(
            data.as_ptr() as *const std::ffi::c_void,
            data.len(),
            None,
            None,
            None,
            entry_point,
            target,
            0,
            0,
            &mut blob as *mut _ as *mut _,
            Some(&mut err_blob),
        ) {
            Ok(_) => Ok(unsafe { blob.assume_init().unwrap() }),
            Err(_) => {
                let err_blob = err_blob.unwrap();
                let errors = err_blob.GetBufferPointer();
                let len = err_blob.GetBufferSize();
                let error_slice = std::slice::from_raw_parts(errors as *const u8, len);
                let err_string = String::from_utf8_lossy(error_slice);

                Err(eyre!("Failed to compile because: {}", err_string))
            }
        }
    }
}
