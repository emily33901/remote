use std::{ops::Deref, time::Duration};

use eyre::Result;
use tokio::sync::mpsc::{self, error::TryRecvError};
use tracing::Instrument;
use util::JoinhandleExt;
use windows::{
    core::Interface,
    Win32::{
        Foundation::{ERROR_ACCESS_DENIED, E_ACCESSDENIED},
        Graphics::{
            Direct3D11::{
                ID3D11Device, ID3D11Texture2D, D3D11_BOX, D3D11_RESOURCE_MISC_FLAG,
                D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_TEXTURE2D_DESC,
            },
            Dxgi::{
                Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_DESC},
                IDXGIAdapter, IDXGIDevice2, IDXGIKeyedMutex, IDXGIOutput1, IDXGIOutputDuplication,
                IDXGIResource, DXGI_ENUM_MODES_DISABLED_STEREO, DXGI_ERROR_ACCESS_DENIED,
                DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_INVALID_CALL, DXGI_ERROR_WAIT_TIMEOUT,
                DXGI_OUTDUPL_DESC, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTPUT_DESC,
            },
        },
    },
};

use crate::{
    conversion,
    encoder::{self, Encoder},
    produce::{MediaControl, MediaEvent},
    texture_pool::{Texture, TexturePool},
    Encoding, EncodingOptions, ARBITRARY_MEDIA_CHANNEL_LIMIT,
};

use super::dx;

pub(crate) enum DDControl {}

pub(crate) enum DDEvent {
    Frame(Texture, crate::Timestamp),
}

#[tracing::instrument]
pub(crate) fn desktop_duplication() -> Result<(mpsc::Sender<DDControl>, mpsc::Receiver<DDEvent>)> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);

    let span = tracing::Span::current();

    tokio::task::spawn_blocking(move || {
        let _span_guard = span.enter();

        struct DesktopDuplicationContext {
            width: u32,
            height: u32,

            device: ID3D11Device,

            duplicated: IDXGIOutputDuplication,
            texture_pool: TexturePool,
        }

        fn make_desktop_duplication() -> windows::core::Result<DesktopDuplicationContext> {
            let (device, _context) = dx::create_device()?;

            let dxgi_device: IDXGIDevice2 = device.cast()?;

            let parent: IDXGIAdapter = unsafe { dxgi_device.GetParent() }?;

            let primary = unsafe { parent.EnumOutputs(0) }?;
            let primary: IDXGIOutput1 = primary.cast()?;

            let mut best_mode = None;
            {
                let primary = primary.clone();
                let display = primary;
                let _desc = DXGI_OUTPUT_DESC::default();

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
                    // tracing::info!("mode {mode:?}");
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

            tracing::info!(?best_mode);

            // https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutput1-duplicateoutput#return-value
            let duplicated = unsafe { primary.DuplicateOutput(&device) }?;

            let mut desc = DXGI_OUTDUPL_DESC::default();
            unsafe { duplicated.GetDesc(&mut desc) };

            tracing::debug!(?desc);

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

            Ok(DesktopDuplicationContext {
                width: width,
                height: height,
                device: device,
                duplicated: duplicated,
                texture_pool: texture_pool,
            })
        }

        let mut control_rx_open = || match control_rx.try_recv() {
            Ok(_) | Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        };

        tracing::debug!("starting");

        scopeguard::defer! {
            tracing::debug!("stopping");
        }

        'top: while control_rx_open() {
            let context = match make_desktop_duplication() {
                Ok(ok) => ok,
                Err(err) => {
                    match err.code() {
                        DXGI_ERROR_ACCESS_DENIED | E_ACCESSDENIED => {
                            tracing::debug!("access denied, retrying in 1s");
                            std::thread::sleep(Duration::from_secs(1));
                            continue;
                        }
                        _ => {
                            tracing::debug!(%err, "unknown error trying to make desktop duplication context");
                            break;
                        }
                    }
                }
            };

            let DesktopDuplicationContext {
                device,
                duplicated,
                height,
                width,
                texture_pool,
            } = context;

            let start_time = std::time::Instant::now();

            while control_rx_open() {
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

                            let out_texture = texture_pool.acquire();

                            // NOTE(emily): Cannot use dx::copy_texture here, input is already acquired.

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
                                        out_texture.deref(),
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

                            // TODO(emily): We should probably allow ourselves to be backpressured here
                            match event_tx.try_send(DDEvent::Frame(
                                out_texture,
                                crate::Timestamp::new_diff_instant(
                                    start_time,
                                    std::time::Instant::now(),
                                ),
                            )) {
                                Ok(_) => {}
                                Err(mpsc::error::TrySendError::Closed(_)) => break,
                                Err(mpsc::error::TrySendError::Full(_)) => {}
                            }
                        }
                    }
                    Err(err) => {
                        // https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-acquirenextframe#return-value
                        match err.code() {
                            DXGI_ERROR_WAIT_TIMEOUT => {
                                continue;
                            }
                            DXGI_ERROR_ACCESS_LOST => {
                                tracing::debug!("access lost, recreating desktop duplication");
                                break;
                            }
                            DXGI_ERROR_INVALID_CALL => {
                                unreachable!("should always release previous frame before attempting to accquire next");
                            }
                            _ => {
                                tracing::error!(%err, "unknown error; shutting down");
                                break 'top;
                            }
                        }
                    }
                }
            }
        }

        eyre::Ok(())
    }).watch(|r| {
        if let Err(err) = r {
            tracing::debug!("failed: {err:?}");
        }
    });

    Ok((control_tx, event_rx))
}

pub async fn duplicate_desktop(
    encoder_api: Encoder,
    encoding: Encoding,
    encoding_options: EncodingOptions,
    width: u32,
    height: u32,
    frame_rate: u32,
) -> Result<(mpsc::Sender<MediaControl>, mpsc::Receiver<MediaEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);

    let (h264_control, mut h264_event) = encoder_api
        .run(width, height, frame_rate, encoding, encoding_options)
        .await?;

    let (convert_control, mut convert_event) = conversion::dxva_converter(
        width,
        height,
        conversion::Format::BGRA,
        conversion::Format::NV12,
    )
    .await?;

    let (dd_control, mut dd_event) = desktop_duplication()?;

    tokio::spawn(async move {
        // Our control keeps the inner dd control
        // This also indirectly keeps the cc control alive by keeping dd event alive
        let _dd_control = dd_control;

        while let Some(_control) = control_rx.recv().await {}

        tracing::debug!("media control gone");
    });

    tokio::spawn(async move {
        match async move {
            while let Some(event) = dd_event.recv().await {
                match event {
                    DDEvent::Frame(texture, time) => {
                        convert_control
                            .send(conversion::ConvertControl::Frame(texture, time))
                            .await?
                    }
                }
            }
            eyre::Ok(())
        }
        .await
        {
            Ok(_) => {}
            Err(err) => tracing::error!("dd event err {err}"),
        }
    });

    tokio::spawn(async move {
        match async move {
            while let Some(event) = convert_event.recv().await {
                match event {
                    conversion::ConvertEvent::Frame(frame, time, statistics) => {
                        h264_control
                            .send(encoder::EncoderControl::Frame(
                                frame,
                                time,
                                crate::Statistics {
                                    encode: None,
                                    decode: None,
                                    convert: Some(statistics),
                                },
                            ))
                            .await?
                    }
                }
            }
            eyre::Ok(())
        }
        .await
        {
            Ok(_) => {}
            Err(err) => tracing::error!("convert event err {err}"),
        }
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            match async move {
                while let Some(event) = h264_event.recv().await {
                    match event {
                        encoder::EncoderEvent::Data(data) => {
                            event_tx.send(MediaEvent::Video(data)).await?
                        }
                    }
                }

                eyre::Ok(())
            }
            .await
            {
                Ok(_) => {}
                Err(err) => tracing::error!("encoder event err {err}"),
            }
        }
    });

    Ok((control_tx, event_rx))
}
