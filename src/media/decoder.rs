use std::mem::MaybeUninit;

use eyre::Result;
use tokio::sync::mpsc;
use windows::{
    core::ComInterface,
    Win32::{
        Graphics::{
            Direct3D11::{
                ID3D11Texture2D, D3D11_BOX, D3D11_RESOURCE_MISC_FLAG,
                D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_TEXTURE2D_DESC,
            },
            Dxgi::{Common::DXGI_FORMAT_NV12, IDXGIKeyedMutex},
        },
        Media::MediaFoundation::*,
        System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE},
    },
};

use crate::{video::VideoBuffer, ARBITRARY_CHANNEL_LIMIT};

use super::dx::copy_texture;

pub(crate) enum DecoderControl {
    Data(VideoBuffer),
}
pub(crate) enum DecoderEvent {
    Frame(ID3D11Texture2D, std::time::SystemTime),
}

pub(crate) async fn h264_decoder(
    width: u32,
    height: u32,
    target_framerate: u32,
    target_bitrate: u32,
) -> Result<(mpsc::Sender<DecoderControl>, mpsc::Receiver<DecoderEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "h264-decoder-control").await;
    telemetry::client::watch_channel(&event_tx, "h264-decoder-event").await;

    tokio::spawn({
        async move {
            match tokio::task::spawn_blocking(move || unsafe {
                CoInitializeEx(None, COINIT_DISABLE_OLE1DDE | COINIT_APARTMENTTHREADED)?;
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

                MFTEnumEx(
                    MFT_CATEGORY_VIDEO_DECODER,
                    MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                    Some(&MFT_REGISTER_TYPE_INFO {
                        guidMajorType: MFMediaType_Video,
                        guidSubtype: MFVideoFormat_H264_ES,
                    } as *const _),
                    None,
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

                attributes.SetUINT32(&CODECAPI_AVLowLatencyMode, 1)?;
                attributes.SetUINT32(&CODECAPI_AVDecNumWorkerThreads, 8)?;
                attributes.SetUINT32(&CODECAPI_AVDecVideoAcceleration_H264, 1)?;
                attributes.SetUINT32(&CODECAPI_AVDecVideoThumbnailGenerationMode, 0)?;
                if attributes.GetUINT32(&MF_SA_D3D11_AWARE)? != 1 {
                    panic!("Not D3D11 aware");
                }

                transform.ProcessMessage(
                    MFT_MESSAGE_SET_D3D_MANAGER,
                    std::mem::transmute(device_manager),
                )?;

                attributes.SetUINT32(&MF_LOW_LATENCY as *const _, 1)?;

                {
                    // let input_type = MFCreateMediaType()?;
                    let input_type = transform.GetInputAvailableType(0, 0)?;

                    input_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    input_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264_ES)?;

                    let width_height = (width as u64) << 32 | (height as u64);
                    input_type.SetUINT64(&MF_MT_FRAME_SIZE, width_height)?;

                    let frame_rate = (target_framerate as u64) << 32 | 1_u64;
                    input_type.SetUINT64(&MF_MT_FRAME_RATE as *const _, frame_rate)?;

                    input_type.SetUINT32(&MF_MT_AVG_BITRATE as *const _, target_bitrate)?;

                    // let pixel_aspect_ratio = (1_u64) << 32 | 1_u64;
                    // input_type
                    //     .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO as *const _, pixel_aspect_ratio)?;

                    input_type
                        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;

                    // input_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;

                    transform.SetInputType(0, &input_type, 0)?;
                }

                {
                    let output_type = transform.GetOutputAvailableType(0, 0)?;
                    // let output_type = MFCreateMediaType()?;
                    output_type.SetGUID(
                        &MF_MT_MAJOR_TYPE as *const _,
                        &MFMediaType_Video as *const _,
                    )?;
                    output_type
                        .SetGUID(&MF_MT_SUBTYPE as *const _, &MFVideoFormat_NV12 as *const _)?;

                    output_type.SetUINT32(&MF_MT_AVG_BITRATE as *const _, target_bitrate)?;

                    let width_height = (width as u64) << 32 | (height as u64);
                    output_type.SetUINT64(&MF_MT_FRAME_SIZE, width_height)?;

                    let frame_rate = (target_framerate as u64) << 32 | 1_u64;
                    output_type.SetUINT64(&MF_MT_FRAME_RATE as *const _, frame_rate)?;

                    let pixel_aspect_ratio = (1_u64) << 32 | 1_u64;
                    output_type
                        .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO as *const _, pixel_aspect_ratio)?;

                    transform.SetOutputType(0, &output_type, 0)?;
                }

                transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
                transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
                transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

                loop {
                    let DecoderControl::Data(VideoBuffer {
                        mut data,
                        sequence_header,
                        time,
                        duration,
                    }) = control_rx
                        .blocking_recv()
                        .ok_or(eyre::eyre!("encoder control closed"))?;

                    if let Some(sequence_header) = sequence_header {
                        let input_type = transform.GetInputCurrentType(0)?;
                        input_type.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &sequence_header)?;
                    }

                    let len = data.len();

                    let media_buffer = MFCreateMemoryBuffer(len as u32)?;

                    let mut begin: MaybeUninit<*mut u8> = MaybeUninit::uninit();
                    media_buffer.Lock(&mut begin as *mut _ as *mut *mut u8, None, None)?;

                    let begin = begin.assume_init();

                    std::ptr::copy(data.as_mut_ptr(), begin, len as usize);

                    media_buffer.SetCurrentLength(len as u32)?;
                    media_buffer.Unlock()?;

                    let sample = MFCreateSample()?;
                    sample.AddBuffer(&media_buffer)?;

                    sample.SetSampleTime(
                        time.duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .unwrap()
                            .as_nanos() as i64
                            / 100,
                    )?;
                    sample.SetSampleDuration(duration.as_nanos() as i64 / 100)?;

                    let process_output = || {
                        // let status = transform.GetOutputStatus()?;

                        // if status != MFT_OUTPUT_STATUS_SAMPLE_READY.0 as u32 {
                        //     // return Err(MF_E_TRANSFORM_NEED_MORE_INPUT);
                        //     return Err(windows::core::Error::new(
                        //         MF_E_TRANSFORM_NEED_MORE_INPUT,
                        //         HSTRING::from("need more input"),
                        //     ));
                        // }

                        let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();
                        output_buffer.dwStatus = 0;
                        output_buffer.dwStreamID = 0;

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

                                // TODO(emily): fake timestamp because h264 has no timestamp
                                event_tx
                                    .blocking_send(DecoderEvent::Frame(
                                        output_texture,
                                        std::time::SystemTime::now(),
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
                        Ok(_) => loop {
                            match process_output().map_err(|err| err.code()) {
                                Ok(_) => {
                                    // log::info!("process output ok");
                                    // break;
                                }
                                Err(MF_E_TRANSFORM_NEED_MORE_INPUT) => {
                                    log::debug!("need more input");
                                    break;
                                }
                                Err(MF_E_TRANSFORM_STREAM_CHANGE) => {
                                    log::warn!("stream change");

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
                                Err(err) => todo!("No idea what to do with {err}"),
                            };
                        },
                        Err(MF_E_NOTACCEPTING) => {
                            log::warn!(
                                "decoder is not accepting frames something has gone horribly wrong"
                            )
                        }
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
