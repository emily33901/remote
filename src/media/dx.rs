use eyre::{eyre, Result};
use windows::{
    core::ComInterface,
    Win32::Graphics::{Direct3D::*, Direct3D11::*, Dxgi::Common::DXGI_FORMAT},
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

pub(crate) fn create_texture(
    device: &ID3D11Device,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
) -> Result<ID3D11Texture2D> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: 0,
        CPUAccessFlags: 0,
        MiscFlags: D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32
            | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 as u32,
    };

    let mut texture: Option<ID3D11Texture2D> = None;

    unsafe {
        device.CreateTexture2D(&description as *const _, None, Some(&mut texture as *mut _))?
    };

    texture.ok_or(eyre!("Unable to create texture"))
}

pub(crate) fn create_texture_sync(
    device: &ID3D11Device,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
) -> Result<ID3D11Texture2D> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: 0,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };

    let mut texture: Option<ID3D11Texture2D> = None;

    unsafe {
        device.CreateTexture2D(&description as *const _, None, Some(&mut texture as *mut _))?
    };

    texture.ok_or(eyre!("Unable to create texture"))
}

pub(crate) fn create_staging_texture_read(
    device: &ID3D11Device,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
) -> Result<ID3D11Texture2D> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 0,
        ArraySize: 1,
        Format: format,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };

    let mut texture: Option<ID3D11Texture2D> = None;

    unsafe {
        device.CreateTexture2D(&description as *const _, None, Some(&mut texture as *mut _))?
    };

    texture.ok_or(eyre!("Unable to create texture"))
}

pub(crate) fn create_staging_texture_write(
    device: &ID3D11Device,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
) -> Result<ID3D11Texture2D> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 0,
        ArraySize: 1,
        Format: format,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
    };

    let mut texture: Option<ID3D11Texture2D> = None;

    unsafe {
        device.CreateTexture2D(&description as *const _, None, Some(&mut texture as *mut _))?
    };

    texture.ok_or(eyre!("Unable to create texture"))
}
