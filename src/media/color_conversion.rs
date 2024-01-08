use std::{mem::MaybeUninit, time::UNIX_EPOCH};

use eyre::Result;
use tokio::sync::mpsc;
use windows::{
    core::ComInterface,
    Win32::{
        Foundation::FALSE,
        Graphics::Direct3D11::ID3D11Texture2D,
        Media::MediaFoundation::*,
        System::Com::{CoInitializeEx, COINIT_DISABLE_OLE1DDE},
    },
};

use crate::ARBITRARY_CHANNEL_LIMIT;

use super::dx::copy_texture;

pub(crate) enum ConvertControl {
    Frame(ID3D11Texture2D, std::time::SystemTime),
}
pub(crate) enum ConvertEvent {
    Frame(ID3D11Texture2D, std::time::SystemTime),
}

#[derive(Copy, Clone)]
pub(crate) enum Format {
    NV12,
    BGRA,
}

impl From<Format> for windows::core::GUID {
    fn from(value: Format) -> Self {
        match value {
            Format::NV12 => MFVideoFormat_NV12,
            Format::BGRA => MFVideoFormat_RGB32,
        }
    }
}

impl From<Format> for super::dx::TextureFormat {
    fn from(value: Format) -> Self {
        match value {
            Format::NV12 => Self::NV12,
            Format::BGRA => Self::BGRA,
        }
    }
}

pub(crate) async fn converter(
    width: u32,
    height: u32,
    input_format: Format,
    output_format: Format,
) -> Result<(mpsc::Sender<ConvertControl>, mpsc::Receiver<ConvertEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "color-converter-control").await;
    telemetry::client::watch_channel(&event_tx, "color-converter-event").await;

    tokio::spawn({
        async move {
            match tokio::task::spawn_blocking(move || unsafe {
                CoInitializeEx(None, COINIT_DISABLE_OLE1DDE)?;
                unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)? }

                let mut reset_token = 0_u32;
                let mut device_manager: Option<IMFDXGIDeviceManager> = None;

                let (device, _context) = super::dx::create_device()?;

                unsafe {
                    MFCreateDXGIDeviceManager(
                        &mut reset_token as *mut _,
                        &mut device_manager as *mut _,
                    )
                }?;

                let device_manager = device_manager.unwrap();

                unsafe { device_manager.ResetDevice(&device, reset_token) }?;

                let mut count = 0_u32;
                let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();

                // TODO(emily): Add software support

                MFTEnumEx(
                    MFT_CATEGORY_VIDEO_PROCESSOR,
                    MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
                    Some(&MFT_REGISTER_TYPE_INFO {
                        guidMajorType: MFMediaType_Video,
                        guidSubtype: input_format.into(),
                    }),
                    Some(&MFT_REGISTER_TYPE_INFO {
                        guidMajorType: MFMediaType_Video,
                        guidSubtype: output_format.into(),
                    }),
                    &mut activates,
                    &mut count,
                )?;

                // TODO(emily): CoTaskMemFree activates

                let activates = std::slice::from_raw_parts_mut(activates, count as usize)
                    .iter()
                    .filter_map(|x| x.as_ref())
                    .collect::<Vec<_>>();

                let activate = activates.first().unwrap();
                let transform: IMFTransform = activate.ActivateObject()?;

                let attributes = transform.GetAttributes()?;

                // attributes.SetUINT32(&CODECAPI_AVLowLatencyMode, 1)?;
                // attributes.SetUINT32(&CODECAPI_AVDecNumWorkerThreads, 8)?;
                // attributes.SetUINT32(&CODECAPI_AVDecVideoAcceleration_H264, 1)?;
                // attributes.SetUINT32(&CODECAPI_AVDecVideoThumbnailGenerationMode, 0)?;
                if attributes.GetUINT32(&MF_SA_D3D11_AWARE)? != 1 {
                    panic!("Not D3D11 aware");
                }

                transform.ProcessMessage(
                    MFT_MESSAGE_SET_D3D_MANAGER,
                    std::mem::transmute(device_manager),
                )?;

                // attributes.SetUINT32(&MF_LOW_LATENCY as *const _, 1)?;

                {
                    // let input_type = MFCreateMediaType()?;
                    let input_type = transform.GetInputAvailableType(0, 0)?;

                    input_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    input_type.SetGUID(&MF_MT_SUBTYPE, &input_format.into())?;

                    let width_height = (width as u64) << 32 | (height as u64);
                    input_type.SetUINT64(&MF_MT_FRAME_SIZE, width_height)?;

                    input_type.SetUINT64(&MF_MT_FRAME_RATE, (30 << 32) | (1))?;

                    input_type
                        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                    input_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
                    // input_type.SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)?;

                    let pixel_aspect_ratio = (1_u64) << 32 | 1_u64;
                    input_type
                        .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO as *const _, pixel_aspect_ratio)?;

                    transform.SetInputType(0, &input_type, 0)?;
                }

                for i in 0.. {
                    if let Ok(output_type) = transform.GetOutputAvailableType(0, i) {
                        let subtype = output_type.GetGUID(&MF_MT_SUBTYPE)?;
                        if subtype == output_format.into() {
                            let width_height = (width as u64) << 32 | (height as u64);
                            output_type.SetUINT64(&MF_MT_FRAME_SIZE, width_height)?;

                            output_type.SetUINT64(&MF_MT_FRAME_RATE, (30 << 32) | (1))?;

                            output_type.SetUINT32(
                                &MF_MT_INTERLACE_MODE,
                                MFVideoInterlace_Progressive.0 as u32,
                            )?;
                            output_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
                            output_type.SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)?;

                            let pixel_aspect_ratio = (1_u64) << 32 | 1_u64;
                            output_type.SetUINT64(
                                &MF_MT_PIXEL_ASPECT_RATIO as *const _,
                                pixel_aspect_ratio,
                            )?;

                            transform.SetOutputType(0, &output_type, 0)?;
                            break;
                        }
                    } else {
                        break;
                    }
                }

                transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
                transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
                transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

                let output_sample_texture = super::dx::TextureBuilder::new(
                    &device,
                    width,
                    height,
                    super::dx::TextureFormat::NV12,
                )
                .build()
                .unwrap();

                let _output_sample = MFCreateVideoSampleFromSurface(&output_sample_texture)?;

                loop {
                    let ConvertControl::Frame(frame, time) = control_rx
                        .blocking_recv()
                        .ok_or(eyre::eyre!("convert control closed"))?;

                    let dxgi_buffer =
                        MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &frame, 0, FALSE)?;

                    let sample = unsafe { MFCreateSample() }?;

                    sample.AddBuffer(&dxgi_buffer)?;

                    // let sample = MFCreateVideoSampleFromSurface(&frame)?;

                    sample
                        .SetSampleTime(time.duration_since(UNIX_EPOCH)?.as_nanos() as i64 / 100)?;
                    sample.SetSampleDuration(100_000_000 / 30 as i64)?;

                    let process_output = || {
                        let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();
                        output_buffer.dwStatus = 0;
                        output_buffer.dwStreamID = 0;
                        // output_buffer.pSample = ManuallyDrop::new(Some(output_sample.clone()));
                        // let stream_output = transform.GetOutputStreamInfo(0)?;
                        // log::info!("{stream_output:?}");

                        let mut output_buffers = [output_buffer];

                        let mut status = 0_u32;
                        match transform.ProcessOutput(0, &mut output_buffers, &mut status) {
                            Ok(ok) => {
                                let output_texture = super::dx::TextureBuilder::new(
                                    &device,
                                    width,
                                    height,
                                    super::dx::TextureFormat::NV12,
                                )
                                .keyed_mutex()
                                .nt_handle()
                                .build()
                                .unwrap();

                                let sample = output_buffers[0].pSample.take().unwrap();
                                let timestamp = unsafe { sample.GetSampleTime()? };

                                let media_buffer = unsafe { sample.GetBufferByIndex(0) }?;
                                let dxgi_buffer: IMFDXGIBuffer = media_buffer.cast()?;

                                let mut texture: MaybeUninit<ID3D11Texture2D> =
                                    MaybeUninit::uninit();

                                unsafe {
                                    dxgi_buffer.GetResource(
                                        &ID3D11Texture2D::IID as *const _,
                                        &mut texture as *mut _ as *mut *mut std::ffi::c_void,
                                    )
                                }?;

                                let subresource_index =
                                    unsafe { dxgi_buffer.GetSubresourceIndex()? };
                                let texture = unsafe { texture.assume_init() };

                                copy_texture(&output_texture, &texture, Some(subresource_index))?;

                                event_tx
                                    .blocking_send(ConvertEvent::Frame(
                                        output_texture,
                                        std::time::SystemTime::UNIX_EPOCH
                                            + std::time::Duration::from_nanos(
                                                timestamp as u64 * 100,
                                            ),
                                    ))
                                    .unwrap();

                                Ok(ok)
                            }
                            Err(err) => {
                                // log::warn!("output flags {}", output_buffers[0].dwStatus);
                                Err(err)
                            }
                        }
                    };

                    match transform
                        .ProcessInput(0, &sample, 0)
                        .map_err(|err| err.code())
                    {
                        Ok(_) => {
                            log::info!("cc process input OK")
                        }
                        Err(MF_E_NOTACCEPTING) => loop {
                            match process_output().map_err(|err| err.code()) {
                                Ok(_) => {
                                    // log::info!("process output ok");
                                    // break;
                                }
                                Err(MF_E_TRANSFORM_NEED_MORE_INPUT) => {
                                    log::debug!("cc need more input");
                                    break;
                                }
                                Err(MF_E_TRANSFORM_STREAM_CHANGE) => {
                                    log::warn!("cc stream change");

                                    {
                                        for i in 0.. {
                                            if let Ok(output_type) =
                                                transform.GetOutputAvailableType(0, i)
                                            {
                                                let subtype =
                                                    output_type.GetGUID(&MF_MT_SUBTYPE)?;

                                                super::produce::debug_video_format(&output_type)?;

                                                if subtype == MFVideoFormat_NV12 {
                                                    transform.SetOutputType(0, &output_type, 0)?;
                                                    break;
                                                }
                                            } else {
                                                break;
                                            }
                                        }
                                    }

                                    // transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
                                }
                                Err(err) => {
                                    log::error!("No idea what to do with {err}");
                                    break;
                                    // todo!("No idea what to do with {err}")
                                }
                            };
                        },
                        Err(err) => todo!("No idea what to do with {err}"),
                    }
                }

                eyre::Ok(())
            })
            .await
            .unwrap()
            {
                Ok(_) => log::warn!("h264::decoder exit Ok"),
                Err(err) => log::error!("h264::decoder exit err {err} {err:?}"),
            }
        }
    });

    Ok((control_tx, event_rx))
}
