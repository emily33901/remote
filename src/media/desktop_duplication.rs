use eyre::Result;
use tokio::sync::mpsc;
use windows::{
    core::ComInterface,
    Win32::{
        Foundation::CloseHandle,
        Graphics::{
            Direct3D11::{
                ID3D11Device1, ID3D11Texture2D, D3D11_BOX, D3D11_RESOURCE_MISC_FLAG,
                D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_TEXTURE2D_DESC,
            },
            Dxgi::{
                Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_MODE_DESC},
                IDXGIAdapter, IDXGIDevice2, IDXGIKeyedMutex, IDXGIOutput1, IDXGIResource,
                IDXGIResource1, DXGI_ENUM_MODES_DISABLED_STEREO, DXGI_ERROR_WAIT_TIMEOUT,
                DXGI_OUTDUPL_DESC, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTPUT_DESC,
                DXGI_SHARED_RESOURCE_WRITE,
            },
        },
        System::Performance::QueryPerformanceCounter,
    },
};

use crate::ARBITRARY_CHANNEL_LIMIT;

use super::dx;

pub(crate) enum DDControl {}

pub(crate) enum DDEvent {
    Size(u32, u32),
    Frame(ID3D11Texture2D, std::time::SystemTime),
}

pub(crate) fn desktop_duplication() -> Result<(mpsc::Sender<DDControl>, mpsc::Receiver<DDEvent>)> {
    let (control_tx, _control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    tokio::spawn(async move {
        match tokio::task::spawn_blocking(move || {
            let (device, _context) = dx::create_device()?;
            let (device2, _context2) = dx::create_device()?;

            let dxgi_device: IDXGIDevice2 = device.cast()?;

            let parent: IDXGIAdapter = unsafe { dxgi_device.GetParent() }?;

            let primary = unsafe { parent.EnumOutputs(0) }?;
            let primary: IDXGIOutput1 = primary.cast()?;

            let mut best_mode = None;
            {
                let primary = primary.clone();
                let display = primary;
                let mut desc = DXGI_OUTPUT_DESC::default();

                let modes = {
                    let mut num_modes = unsafe {
                        let mut num_modes = 0;
                        display.GetDisplayModeList(
                            DXGI_FORMAT_B8G8R8A8_UNORM,
                            DXGI_ENUM_MODES_DISABLED_STEREO,
                            &mut num_modes,
                            None,
                        )?;

                        num_modes
                    };

                    let mut modes: Vec<DXGI_MODE_DESC> =
                        vec![DXGI_MODE_DESC::default(); num_modes as usize];

                    unsafe {
                        display.GetDisplayModeList(
                            DXGI_FORMAT_B8G8R8A8_UNORM,
                            DXGI_ENUM_MODES_DISABLED_STEREO,
                            &mut num_modes,
                            Some(modes.as_mut_ptr()),
                        )?;
                    }
                    modes
                };

                for mode in modes {
                    // log::info!("mode {mode:?}");
                    if best_mode.is_none() {
                        best_mode = Some(mode);
                        continue;
                    }

                    let best_mode_desc = best_mode.as_ref().unwrap();
                    if mode.Width > best_mode_desc.Width
                        || mode.Height > best_mode_desc.Height
                        || (mode.Width >= best_mode_desc.Width
                            && mode.Height >= best_mode_desc.Height
                            && (mode.RefreshRate.Numerator as f32
                                / mode.RefreshRate.Denominator as f32)
                                > (best_mode_desc.RefreshRate.Numerator as f32
                                    / best_mode_desc.RefreshRate.Denominator as f32))
                    {
                        best_mode = Some(mode);
                    }
                }
            }

            log::info!("best mode would be {best_mode:?}");

            let duplicated = unsafe { primary.DuplicateOutput(&device) }?;

            let mut desc = DXGI_OUTDUPL_DESC::default();
            unsafe { duplicated.GetDesc(&mut desc) };

            log::info!("output desc {desc:?}");

            let (device_width, device_height) = (desc.ModeDesc.Width, desc.ModeDesc.Height);

            event_tx.blocking_send(DDEvent::Size(desc.ModeDesc.Height, desc.ModeDesc.Height))?;

            let mut start = 0;
            unsafe { QueryPerformanceCounter(&mut start) }?;
            let _start_time = std::time::SystemTime::now();

            loop {
                let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
                let mut frame_resource: Option<IDXGIResource> = None;

                unsafe {
                    let _ = duplicated.ReleaseFrame();
                }

                match unsafe {
                    duplicated.AcquireNextFrame(1000, &mut frame_info, &mut frame_resource)
                } {
                    Ok(_) => {
                        if frame_info.AccumulatedFrames == 0 || frame_info.LastPresentTime == 0 {
                            // Only mouse moved
                        } else {
                            let frame_resource = frame_resource.unwrap();
                            let duplication_texture: ID3D11Texture2D = frame_resource.cast()?;

                            let out_texture = dx::TextureBuilder::new(
                                &device,
                                device_width,
                                device_height,
                                dx::TextureFormat::BGRA,
                            )
                            .nt_handle()
                            .keyed_mutex()
                            .build()?;

                            // NOTE(emily): Cannot use dx::copy_texture here because whilst the input might appear
                            // to be keyed mutex, it is impossible to actually acquire that.

                            {
                                let mut in_desc = D3D11_TEXTURE2D_DESC::default();
                                let mut out_desc = D3D11_TEXTURE2D_DESC::default();
                                unsafe {
                                    duplication_texture.GetDesc(&mut in_desc as *mut _);
                                    out_texture.GetDesc(&mut out_desc as *mut _);
                                }

                                let keyed_out =
                                    if D3D11_RESOURCE_MISC_FLAG(out_desc.MiscFlags as i32)
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

                                scopeguard::defer! {
                                    if let Some(keyed) = keyed_out {
                                        unsafe {
                                            let _ = keyed.ReleaseSync(0);
                                        }
                                    }
                                }

                                let device = unsafe { duplication_texture.GetDevice() }?;
                                let context = unsafe { device.GetImmediateContext() }?;

                                let region = D3D11_BOX {
                                    left: 0,
                                    top: 0,
                                    front: 0,
                                    right: out_desc.Width,
                                    bottom: out_desc.Height,
                                    back: 1,
                                };

                                let subresource_index = 0;

                                unsafe {
                                    context.CopySubresourceRegion(
                                        &out_texture,
                                        0,
                                        0,
                                        0,
                                        0,
                                        &duplication_texture,
                                        subresource_index,
                                        Some(&region),
                                    )
                                };
                            }

                            match event_tx.try_send(DDEvent::Frame(
                                out_texture,
                                std::time::SystemTime::now() + std::time::Duration::from_millis(50),
                            )) {
                                Ok(_) => {}
                                Err(mpsc::error::TrySendError::Closed(_)) => break,
                                Err(mpsc::error::TrySendError::Full(_)) => {}
                            }
                        }
                    }
                    Err(err) => {
                        match err.code() {
                            DXGI_ERROR_WAIT_TIMEOUT => {
                                continue;
                            }
                            _ => {}
                        }
                        log::error!("desktop duplication error: {err} {err:?}");
                        break;
                    }
                }
            }

            eyre::Ok(())
        })
        .await
        .unwrap()
        {
            Ok(_) => log::warn!("media::desktop_duplication::desktop_duplication exit Ok"),
            Err(err) => log::error!(
                "media::desktop_duplication::desktop_duplication exit err {err} {err:?}"
            ),
        }
    });

    Ok((control_tx, event_rx))
}
