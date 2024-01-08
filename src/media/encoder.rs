use std::time::UNIX_EPOCH;

use eyre::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use windows::{
    core::ComInterface,
    Win32::{
        Foundation::FALSE,
        Graphics::Direct3D11::ID3D11Texture2D,
        Media::MediaFoundation::*,
        System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE},
    },
};

use crate::media::dx::{TextureCPUAccess, TextureUsage};

use crate::{media::produce::debug_video_format, video::VideoBuffer, ARBITRARY_CHANNEL_LIMIT};

use super::dx::MapTextureExt;

use windows::Win32::Graphics::Direct3D11::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) enum FrameIsKeyframe {
    Yes,
    No,
    Perhaps,
}

pub(crate) enum EncoderControl {
    Frame(ID3D11Texture2D, std::time::SystemTime),
}
pub(crate) enum EncoderEvent {
    Data(VideoBuffer),
}

pub(crate) async fn h264_encoder(
    width: u32,
    height: u32,
    target_framerate: u32,
    target_bitrate: u32,
) -> Result<(mpsc::Sender<EncoderControl>, mpsc::Receiver<EncoderEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    tokio::spawn({
        async move {
            match tokio::task::spawn_blocking(move || unsafe {
                CoInitializeEx(None, COINIT_DISABLE_OLE1DDE | COINIT_APARTMENTTHREADED)?;
                unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)? }

                let mut reset_token = 0_u32;
                let mut device_manager: Option<IMFDXGIDeviceManager> = None;

                let (device, context) = super::dx::create_device()?;

                unsafe {
                    MFCreateDXGIDeviceManager(
                        &mut reset_token as *mut _,
                        &mut device_manager as *mut _,
                    )
                }?;

                let device_manager = device_manager.unwrap();

                unsafe { device_manager.ResetDevice(&device, reset_token) }?;

                // TODO(emily): Its not quite this easy because hardware -> async and non-hardware -> sync
                // so we need different code paths here

                let find_encoder = |hardware| {
                    let mut count = 0_u32;
                    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();

                    MFTEnumEx(
                        MFT_CATEGORY_VIDEO_ENCODER,
                        if hardware {
                            MFT_ENUM_FLAG_HARDWARE
                        } else {
                            MFT_ENUM_FLAG(0)
                        } | MFT_ENUM_FLAG_SORTANDFILTER,
                        None,
                        Some(&MFT_REGISTER_TYPE_INFO {
                            guidMajorType: MFMediaType_Video,
                            guidSubtype: MFVideoFormat_H264,
                        } as *const _),
                        &mut activates,
                        &mut count,
                    )?;

                    // TODO(emily): CoTaskMemFree activates

                    let activates = std::slice::from_raw_parts_mut(activates, count as usize);
                    let activate = activates.first().ok_or_else(|| eyre::eyre!("No encoders"))?;

                    // NOTE(emily): If there is an activate then it should be real
                    let activate = activate.as_ref().unwrap();

                    let transform: IMFTransform = activate.ActivateObject()?;

                    eyre::Ok(transform)
                };

                let transform = match find_encoder(true) {
                    Ok(encoder) => encoder,
                    Err(err) => {
                        log::warn!("unable to find a hardware h264 encoder {err}, falling back to a software encoder");
                        find_encoder(false)?
                    }
                };

                let attributes = transform.GetAttributes()?;

                attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;

                let mut input_stream_ids = [0];
                let mut output_stream_ids = [0];

                // NOTE(emily): If this fails then stream ids are 0, 0 anyway
                let _ = transform.GetStreamIDs(&mut input_stream_ids, &mut output_stream_ids);

                // NOTE(emily): If this fails then this is a sofware encoder
                let _is_hardware_transform = transform.ProcessMessage(
                    MFT_MESSAGE_SET_D3D_MANAGER,
                    std::mem::transmute(device_manager),
                ).map(|_| true).unwrap_or_default();

                attributes.SetUINT32(&MF_LOW_LATENCY as *const _, 1)?;

                {
                    let output_type = MFCreateMediaType()?;
                    output_type.SetGUID(
                        &MF_MT_MAJOR_TYPE,
                        &MFMediaType_Video,
                    )?;
                    output_type
                        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;

                    output_type.SetUINT32(&MF_MT_AVG_BITRATE, target_bitrate)?;

                    let width_height = (width as u64) << 32 | (height as u64);
                    output_type.SetUINT64(&MF_MT_FRAME_SIZE, width_height)?;

                    let frame_rate = (target_framerate as u64) << 32 | 1_u64;
                    output_type.SetUINT64(&MF_MT_FRAME_RATE, frame_rate)?;

                    output_type
                        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                    // output_type.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, 100)?;
                    output_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;

                    output_type
                        .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;

                    let pixel_aspect_ratio = (1_u64) << 32 | 1_u64;
                    output_type
                        .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pixel_aspect_ratio)?;

                    debug_video_format(&output_type)?;

                    transform.SetOutputType(output_stream_ids[0], &output_type, 0)?;
                }

                {
                    let input_type = transform.GetInputAvailableType(input_stream_ids[0], 0)?;

                    input_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    input_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;

                    let width_height = (width as u64) << 32 | (height as u64);
                    input_type.SetUINT64(&MF_MT_FRAME_SIZE, width_height)?;

                    let frame_rate = (target_framerate as u64) << 32 | 1_u64;
                    input_type.SetUINT64(&MF_MT_FRAME_RATE, frame_rate)?;

                    debug_video_format(&input_type)?;

                    transform.SetInputType(input_stream_ids[0], &input_type, 0)?;
                }

                // let output_sample = unsafe { MFCreateSample() }?;

                // let output_media_buffer = unsafe { MFCreateMemoryBuffer(width * height * 4) }?;
                // unsafe { output_sample.AddBuffer(&output_media_buffer.clone()) }?;

                let _status = 0;

                let input_stream_id = input_stream_ids[0];
                let output_stream_id = output_stream_ids[0];

                if let Ok(event_gen) = transform.cast::<IMFMediaEventGenerator>() {
                    hardware(event_gen, transform, control_rx, event_tx, target_framerate, output_stream_id, input_stream_id)?;
                } else {
                    software(&device, &context, transform, control_rx, event_tx, target_framerate, output_stream_id, input_stream_id, width, height)?;
                }


                eyre::Ok(())
            })
            .await
            .unwrap()
            {
                Ok(_) => log::warn!("h264::encoder exit Ok"),
                Err(err) => log::error!("h264::encoder exit err {err} {err:?}"),
            }
        }
    });

    Ok((control_tx, event_rx))
}

unsafe fn hardware(
    event_gen: IMFMediaEventGenerator,
    transform: IMFTransform,
    mut control_rx: mpsc::Receiver<EncoderControl>,
    event_tx: mpsc::Sender<EncoderEvent>,
    target_framerate: u32,
    output_stream_id: u32,
    input_stream_id: u32,
) -> eyre::Result<()> {
    transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
    transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
    transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

    loop {
        let event = event_gen.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))?;
        let event_type = event.GetType()?;

        match event_type {
            601 => {
                let EncoderControl::Frame(frame, time) = control_rx
                    .blocking_recv()
                    .ok_or(eyre::eyre!("encoder control closed"))?;

                {
                    let dxgi_buffer =
                        MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &frame, 0, FALSE)?;

                    let sample = unsafe { MFCreateSample() }?;

                    sample.AddBuffer(&dxgi_buffer)?;
                    sample
                        .SetSampleTime(time.duration_since(UNIX_EPOCH)?.as_nanos() as i64 / 100)?;
                    sample.SetSampleDuration(100_000_000 / target_framerate as i64)?;

                    transform.ProcessInput(input_stream_id, &sample, 0)?;
                }
            }

            // METransformHaveOutput
            602 => {
                let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();
                // output_buffer.pSample =
                // ManuallyDrop::new(Some(output_sample.clone())); //  ManuallyDrop::new(Some(sample.clone()));
                output_buffer.dwStatus = 0;
                output_buffer.dwStreamID = output_stream_id;

                let mut output_buffers = [output_buffer];

                let mut status = 0_u32;
                match transform.ProcessOutput(0, &mut output_buffers, &mut status) {
                    Ok(_ok) => {
                        // let timestamp = unsafe { sample.GetSampleTime()? };
                        let sample = output_buffers[0].pSample.take().unwrap();
                        let media_buffer = unsafe { sample.ConvertToContiguousBuffer() }?;

                        // let sample = &output_sample;
                        // let media_buffer = &output_media_buffer;

                        let mut output = vec![];

                        let sample_time;
                        let duration;

                        super::mf::with_locked_media_buffer(&media_buffer, |data, len| {
                            output.extend_from_slice(&data[..*len]);
                            Ok(())
                        })?;

                        let is_keyframe = match sample.GetUINT32(&MFSampleExtension_CleanPoint) {
                            Ok(1) => FrameIsKeyframe::Yes,
                            Ok(0) => FrameIsKeyframe::No,
                            _ => FrameIsKeyframe::Perhaps,
                        };

                        // log::info!("is_keyframe {is_keyframe:?}");

                        // let is_keyframe = sample.GetUINT32(&MFSampleExtension_CleanPoint)? == 1;
                        // let is_keyframe = false;
                        unsafe {
                            sample_time = match sample.GetSampleTime() {
                                Ok(sample_time) => sample_time as u64,
                                Err(_err) => {
                                    // No sample time, but thats fine! we can just throw this sample
                                    log::info!("throwing encoder output sample with no sample time attached");
                                    return Ok(());
                                }
                            };

                            duration = std::time::Duration::from_nanos(
                                sample.GetSampleDuration()? as u64 * 100,
                            );
                        };

                        event_tx.blocking_send(EncoderEvent::Data(VideoBuffer {
                            data: output,
                            sequence_header: None,
                            time: std::time::UNIX_EPOCH
                                + std::time::Duration::from_nanos(sample_time * 100),
                            duration: duration,
                            key_frame: is_keyframe,
                        }))?;
                    }
                    Err(_) => {}
                }
            }

            _ => {
                log::warn!("unknown event {event_type}")
            }
        }
    }
}

unsafe fn software(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    transform: IMFTransform,
    mut control_rx: mpsc::Receiver<EncoderControl>,
    event_tx: mpsc::Sender<EncoderEvent>,
    target_framerate: u32,
    output_stream_id: u32,
    input_stream_id: u32,
    width: u32,
    height: u32,
) -> eyre::Result<()> {
    let staging_texture =
        super::dx::TextureBuilder::new(device, width, height, super::dx::TextureFormat::NV12)
            .usage(TextureUsage::Staging)
            .cpu_access(TextureCPUAccess::Read)
            .build()?;

    transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
    transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

    loop {
        let EncoderControl::Frame(frame, time) = control_rx
            .blocking_recv()
            .ok_or(eyre::eyre!("encoder control closed"))?;

        {
            // Map frame to memory and write to buffer
            super::dx::copy_texture(&staging_texture, &frame, None)?;

            let media_buffer = MFCreateMemoryBuffer(width * height * 2)?;
            staging_texture.map(context, |texture_data| {
                super::mf::with_locked_media_buffer(&media_buffer, |data, len| {
                    data.copy_from_slice(texture_data);
                    *len = texture_data.len();
                    Ok(())
                })?;

                Ok(())
            })?;

            let sample = unsafe { MFCreateSample() }?;

            sample.AddBuffer(&media_buffer)?;
            sample.SetSampleTime(time.duration_since(UNIX_EPOCH)?.as_nanos() as i64 / 100)?;
            sample.SetSampleDuration(100_000_000 / target_framerate as i64)?;

            let process_output = || {
                let output_stream_info = transform.GetOutputStreamInfo(output_stream_id)?;

                let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();

                let output_sample = MFCreateSample()?;
                let media_buffer = MFCreateMemoryBuffer(output_stream_info.cbSize)?;
                output_sample.AddBuffer(&media_buffer)?;

                output_buffer.pSample = std::mem::ManuallyDrop::new(Some(output_sample));
                output_buffer.dwStatus = 0;
                output_buffer.dwStreamID = output_stream_id;

                let mut output_buffers = [output_buffer];

                let mut status = 0_u32;

                // TODO(emily): Copy pasted from above please roll up!

                match transform.ProcessOutput(0, &mut output_buffers, &mut status) {
                    Ok(ok) => {
                        // let timestamp = unsafe { sample.GetSampleTime()? };
                        let sample = output_buffers[0].pSample.take().unwrap();
                        let media_buffer = unsafe { sample.ConvertToContiguousBuffer() }?;

                        // let sample = &output_sample;
                        // let media_buffer = &output_media_buffer;

                        let mut output = vec![];
                        let sequence_header;

                        let sample_time;
                        let duration;

                        super::mf::with_locked_media_buffer(&media_buffer, |data, len| {
                            output.extend_from_slice(&data[..*len]);
                            Ok(())
                        })
                        .unwrap();

                        let is_keyframe = match sample.GetUINT32(&MFSampleExtension_CleanPoint) {
                            Ok(1) => FrameIsKeyframe::Yes,
                            Ok(0) => FrameIsKeyframe::No,
                            _ => FrameIsKeyframe::Perhaps,
                        };

                        unsafe {
                            sample_time = sample.GetUINT64(&MFSampleExtension_DecodeTimestamp)?;

                            duration = std::time::Duration::from_nanos(
                                sample.GetSampleDuration()? as u64 * 100,
                            );

                            sequence_header = if let FrameIsKeyframe::Yes = is_keyframe {
                                // log::info!("keyframe!");
                                let output_type =
                                    transform.GetOutputCurrentType(output_stream_id)?;
                                let extra_data_size =
                                    output_type.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER)? as usize;

                                let mut sequence_header = vec![0; extra_data_size];

                                output_type.GetBlob(
                                    &MF_MT_MPEG_SEQUENCE_HEADER,
                                    &mut sequence_header.as_mut_slice()[..extra_data_size],
                                    None,
                                )?;

                                Some(sequence_header)
                            } else {
                                None
                            };
                        };

                        event_tx
                            .blocking_send(EncoderEvent::Data(VideoBuffer {
                                data: output,
                                sequence_header: sequence_header,
                                time: std::time::UNIX_EPOCH
                                    + std::time::Duration::from_nanos(sample_time * 100),
                                duration: duration,
                                key_frame: is_keyframe,
                            }))
                            .unwrap();

                        Ok(ok)
                    }

                    Err(err) => Err(err),
                }
            };

            match transform
                .ProcessInput(input_stream_id, &sample, 0)
                .map_err(|err| err.code())
            {
                Ok(()) => loop {
                    match process_output().map_err(|err| err.code()) {
                        Ok(_) => {}
                        Err(MF_E_TRANSFORM_NEED_MORE_INPUT) => {
                            log::debug!("need more input");
                            break;
                        }
                        Err(MF_E_TRANSFORM_STREAM_CHANGE) => {
                            log::warn!("stream change");

                            {
                                for i in 0.. {
                                    if let Ok(output_type) = transform.GetOutputAvailableType(0, i)
                                    {
                                        let subtype = output_type.GetGUID(&MF_MT_SUBTYPE)?;

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
                        }

                        Err(err) => todo!("encoder process_output: No idea what to do with {err}"),
                    }
                },
                Err(MF_E_NOTACCEPTING) => {
                    log::warn!("encoder is not accepting frames something has gone horribly wrong")
                }
                Err(err) => todo!("No idea what to do with {err}"),
            }
        }
    }

    eyre::Ok(())
}
