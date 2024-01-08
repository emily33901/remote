use core::slice;
use std::mem::MaybeUninit;

use windows::{
    core::{IUnknown, PWSTR},
    Win32::{
        Graphics::Direct3D11::ID3D11Device,
        Media::MediaFoundation::{
            IMFAttributes, IMFDXGIDeviceManager, IMFMediaBuffer, IMFMediaType, MFCreateAttributes,
            MFCreateDXGIDeviceManager, MFCreateMediaType, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
        },
        System::Com::CoTaskMemFree,
    },
};

use eyre::Result;

pub(crate) fn with_locked_media_buffer<F: FnOnce(&mut [u8], &mut usize) -> Result<()>>(
    buffer: &IMFMediaBuffer,
    f: F,
) -> Result<()> {
    unsafe {
        let mut begin: MaybeUninit<*mut u8> = MaybeUninit::uninit();
        let mut len = buffer.GetCurrentLength()?;
        let mut max_len = buffer.GetMaxLength()?;

        buffer.Lock(
            &mut begin as *mut _ as *mut *mut u8,
            Some(&mut max_len),
            Some(&mut len),
        )?;

        scopeguard::defer! { buffer.Unlock().unwrap() };

        let mut len = len as usize;
        let max_len = max_len as usize;

        let mut s = std::slice::from_raw_parts_mut(begin.assume_init(), max_len);
        f(&mut s, &mut len)?;

        buffer.SetCurrentLength(len as u32)?;
    }

    Ok(())
}

pub(crate) fn debug_video_format(typ: &IMFMediaType) -> Result<()> {
    let width_height = unsafe { typ.GetUINT64(&MF_MT_FRAME_SIZE) }?;

    let width = width_height >> 32 & 0xFFFF_FFFF;
    let height = width_height & 0xFFFF_FFFF;

    let fps_num_denom = unsafe { typ.GetUINT64(&MF_MT_FRAME_RATE) }?;
    let numerator = fps_num_denom >> 32 & 0xFFFF_FFFF;
    let denominator = fps_num_denom & 0xFFFF_FFFF;
    let fps = (numerator as f32) / (denominator as f32);

    log::info!("mf::debug_video_format: {width}x{height} @ {fps} fps");

    Ok(())
}

pub(crate) fn create_dxgi_manager(
    device: &ID3D11Device,
) -> windows::core::Result<IMFDXGIDeviceManager> {
    let mut reset_token = 0_u32;
    let mut device_manager: Option<IMFDXGIDeviceManager> = None;

    unsafe {
        MFCreateDXGIDeviceManager(&mut reset_token as *mut _, &mut device_manager as *mut _)
    }?;

    let device_manager = device_manager.unwrap();

    unsafe { device_manager.ResetDevice(device, reset_token) }?;

    Ok(device_manager)
}

pub(crate) fn create_attributes() -> windows::core::Result<IMFAttributes> {
    let mut attributes: Option<IMFAttributes> = None;

    unsafe { MFCreateAttributes(&mut attributes as *mut _, 2) }?;
    Ok(attributes.unwrap())
}

pub(crate) fn create_media_type() -> windows::core::Result<IMFMediaType> {
    unsafe { MFCreateMediaType() }
}

pub(crate) trait IMFAttributesExt {
    fn get_u32(&self, key: &windows::core::GUID) -> windows::core::Result<u32>;
    fn set_u32(&self, key: &windows::core::GUID, value: u32) -> windows::core::Result<()>;

    fn get_u64(&self, key: &windows::core::GUID) -> windows::core::Result<u64>;
    fn set_u64(&self, key: &windows::core::GUID, value: u64) -> windows::core::Result<()>;

    fn set_fraction(
        &self,
        key: &windows::core::GUID,
        top: u32,
        bottom: u32,
    ) -> windows::core::Result<()>;

    fn get_guid(&self, key: &windows::core::GUID) -> windows::core::Result<windows::core::GUID>;
    fn set_guid(
        &self,
        key: &windows::core::GUID,
        value: &windows::core::GUID,
    ) -> windows::core::Result<()>;

    fn get_string(&self, key: &windows::core::GUID) -> windows::core::Result<String>;

    fn set_unknown<T: windows::core::IntoParam<IUnknown>>(
        &self,
        key: &windows::core::GUID,
        value: T,
    ) -> windows::core::Result<()>;
}

impl IMFAttributesExt for IMFAttributes {
    fn get_u32(&self, key: &windows::core::GUID) -> windows::core::Result<u32> {
        unsafe { self.GetUINT32(key) }
    }

    fn set_u32(&self, key: &windows::core::GUID, value: u32) -> windows::core::Result<()> {
        unsafe { self.SetUINT32(key, value) }
    }

    fn get_u64(&self, key: &windows::core::GUID) -> windows::core::Result<u64> {
        unsafe { self.GetUINT64(key) }
    }

    fn set_u64(&self, key: &windows::core::GUID, value: u64) -> windows::core::Result<()> {
        unsafe { self.SetUINT64(key, value) }
    }

    fn get_guid(&self, key: &windows::core::GUID) -> windows::core::Result<windows::core::GUID> {
        unsafe { self.GetGUID(key) }
    }

    fn set_guid(
        &self,
        key: &windows::core::GUID,
        value: &windows::core::GUID,
    ) -> windows::core::Result<()> {
        unsafe { self.SetGUID(key, value) }
    }

    fn set_fraction(
        &self,
        key: &windows::core::GUID,
        top: u32,
        bottom: u32,
    ) -> windows::core::Result<()> {
        let frac = (top as u64) << 32 | (bottom as u64);
        self.set_u64(key, frac)
    }

    fn get_string(&self, key: &windows::core::GUID) -> windows::core::Result<String> {
        let mut str = PWSTR::null();
        let mut len = 0;

        unsafe {
            self.GetAllocatedString(key, &mut str, &mut len)?;
        }

        scopeguard::defer!(unsafe { CoTaskMemFree(Some(str.as_ptr() as *const _)) });

        let slice = unsafe { std::slice::from_raw_parts(str.as_ptr(), len as usize) };

        Ok(String::from_utf16_lossy(slice))
    }

    fn set_unknown<T: windows::core::IntoParam<IUnknown>>(
        &self,
        key: &windows::core::GUID,
        value: T,
    ) -> windows::core::Result<()> {
        unsafe { self.SetUnknown(key, value) }
    }
}

// TODO(emily): c+p from above, find some better way of doing this similar to impl<T> IMFAttributesExt for T { }
impl IMFAttributesExt for IMFMediaType {
    fn get_u32(&self, key: &windows::core::GUID) -> windows::core::Result<u32> {
        unsafe { self.GetUINT32(key) }
    }

    fn set_u32(&self, key: &windows::core::GUID, value: u32) -> windows::core::Result<()> {
        unsafe { self.SetUINT32(key, value) }
    }

    fn get_u64(&self, key: &windows::core::GUID) -> windows::core::Result<u64> {
        unsafe { self.GetUINT64(key) }
    }

    fn set_u64(&self, key: &windows::core::GUID, value: u64) -> windows::core::Result<()> {
        unsafe { self.SetUINT64(key, value) }
    }

    fn get_guid(&self, key: &windows::core::GUID) -> windows::core::Result<windows::core::GUID> {
        unsafe { self.GetGUID(key) }
    }

    fn set_guid(
        &self,
        key: &windows::core::GUID,
        value: &windows::core::GUID,
    ) -> windows::core::Result<()> {
        unsafe { self.SetGUID(key, value) }
    }

    fn set_fraction(
        &self,
        key: &windows::core::GUID,
        top: u32,
        bottom: u32,
    ) -> windows::core::Result<()> {
        let frac = (top as u64) << 32 | (bottom as u64);
        self.set_u64(key, frac)
    }

    fn get_string(&self, key: &windows::core::GUID) -> windows::core::Result<String> {
        let mut str = PWSTR::null();
        let mut len = 0;

        unsafe {
            self.GetAllocatedString(key, &mut str, &mut len)?;
        }

        scopeguard::defer!(unsafe { CoTaskMemFree(Some(str.as_ptr() as *const _)) });

        let slice = unsafe { std::slice::from_raw_parts(str.as_ptr(), len as usize) };

        Ok(String::from_utf16_lossy(slice))
    }

    fn set_unknown<T: windows::core::IntoParam<IUnknown>>(
        &self,
        key: &windows::core::GUID,
        value: T,
    ) -> windows::core::Result<()> {
        unsafe { self.SetUnknown(key, value) }
    }
}
