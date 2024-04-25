use std::time::UNIX_EPOCH;

use ::windows::{core::Interface, Win32::Media::MediaFoundation::*};
use eyre::{eyre, Result};

use tokio::sync::mpsc::{self};

use crate::{
    dx::{self, TextureCPUAccess, TextureUsage},
    mf::make_dxgi_sample,
};

use crate::{mf::debug_video_format, VideoBuffer, ARBITRARY_CHANNEL_LIMIT};

use crate::{
    dx::MapTextureExt,
    mf::{self, IMFAttributesExt},
};

use ::windows::Win32::Graphics::Direct3D11::*;

use super::{EncoderControl, EncoderEvent};

pub async fn h264_encoder(
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
                mf::init()?;

                let (device, context) = dx::create_device()?;

                let device_manager = crate::mf::create_dxgi_manager(&device)?;

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
                        Some(&MFT_REGISTER_TYPE_INFO {
                            guidMajorType: MFMediaType_Video,
                            guidSubtype: MFVideoFormat_NV12,
                        } as *const _),
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

                    let attributes: IMFAttributes = activate.cast().unwrap();
                    if let Ok(s) = attributes.get_string(&MFT_FRIENDLY_NAME_Attribute) {
                        tracing::info!("chose encoder {s}");
                    }

                    eyre::Ok(transform)
                };

                let transform = match find_encoder(true) {
                    Ok(encoder) => encoder,
                    Err(err) => {
                        tracing::warn!("unable to find a hardware h264 encoder {err}, falling back to a software encoder");
                        find_encoder(false)?
                    }
                };

                let attributes = transform.GetAttributes()?;

                attributes.set_u32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;

                let mut input_stream_ids = [0];
                let mut output_stream_ids = [0];

                // NOTE(emily): If this fails then stream ids are 0, 0 anyway
                let _ = transform.GetStreamIDs(&mut input_stream_ids, &mut output_stream_ids);

                // NOTE(emily): If this fails then this is a sofware encoder
                let _is_hardware_transform = transform.ProcessMessage(
                    MFT_MESSAGE_SET_D3D_MANAGER,
                    std::mem::transmute(device_manager),
                ).map(|_| true).unwrap_or_default();

                attributes.set_u32(&MF_LOW_LATENCY, 1)?;

                {
                    let output_type = MFCreateMediaType()?;
                    output_type.set_guid(
                        &MF_MT_MAJOR_TYPE,
                        &MFMediaType_Video,
                    )?;
                    output_type
                        .set_guid(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;

                    output_type.set_u32(&MF_MT_AVG_BITRATE, target_bitrate)?;

                    output_type.set_fraction(&MF_MT_FRAME_SIZE, width, height)?;
                    output_type.set_fraction(&MF_MT_FRAME_RATE, target_framerate, 1)?;
                    // output_type.set_fraction(&MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;

                    output_type
                        .set_u32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                    // output_type.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, 100)?;
                    // output_type.set_u32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;

                    output_type
                        .set_u32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;

                    debug_video_format(&output_type)?;

                    transform.SetOutputType(output_stream_ids[0], &output_type, 0)?;
                }

                {
                    let input_type = MFCreateMediaType()?;
                    // let input_type = transform.GetInputAvailableType(input_stream_ids[0], 0)?;

                    input_type.set_guid(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    input_type.set_guid(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;

                    input_type.set_fraction(&MF_MT_FRAME_SIZE, width, height)?;
                    input_type.set_fraction(&MF_MT_FRAME_RATE, target_framerate, 1)?;

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
                    tracing::debug!("starting hardware encoder");
                    hardware(&device, event_gen, transform, control_rx, event_tx, target_framerate, output_stream_id, input_stream_id, width, height)?;
                } else {
                    software(&device, &context, transform, control_rx, event_tx, target_framerate, output_stream_id, input_stream_id, width, height)?;
                }


                eyre::Ok(())
            })
            .await
            .unwrap()
            {
                Ok(_) => tracing::warn!("h264::encoder exit Ok"),
                Err(err) => tracing::error!("h264::encoder exit err {err} {err:?}"),
            }
        }
    });

    Ok((control_tx, event_rx))
}

unsafe fn hardware(
    device: &ID3D11Device,
    event_gen: IMFMediaEventGenerator,
    transform: IMFTransform,
    mut control_rx: mpsc::Receiver<EncoderControl>,
    event_tx: mpsc::Sender<EncoderEvent>,
    target_framerate: u32,
    output_stream_id: u32,
    input_stream_id: u32,
    width: u32,
    height: u32,
) -> eyre::Result<()> {
    transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
    transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
    transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

    scopeguard::defer! {
        tracing::debug!("h264 encoder going down");
    };

    // TODO(emily): Consider
    // https://stackoverflow.com/questions/59051443/gop-setting-is-not-honored-by-intel-h264-hardware-mft

    // NOTE(emily): In order to appease the encoder, we need to provide it with a constant stream of tetxures
    // whenever it asks for one. So keep the last control around so that we can use it again if needed.
    let mut last_control = None;

    loop {
        let event = event_gen.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))?;
        let event_type = event.GetType()?;

        // tracing::info!("encoder event {event_type}");

        match event_type {
            601 => {
                let EncoderControl::Frame(frame, time) = {
                    let mut control = None;

                    while let Ok(new_control) = control_rx.try_recv() {
                        control = Some(new_control);
                    }

                    // If we didn't get a frame then wait for one now
                    if control.is_none() && last_control.is_none() {
                        control = Some(
                            control_rx
                                .blocking_recv()
                                .ok_or(eyre!("encoder control closed"))?,
                        );
                    } else if control.is_none() {
                        control = Some(last_control.clone().unwrap());
                    };

                    control.unwrap()
                };

                // TODO(emily): I don't necessarily know that this is correct here. This time could be ahead of the
                // next frame that we get from color conversion and we have no way of knowing. This requires slightly
                // more thought.
                last_control = Some(EncoderControl::Frame(
                    frame.clone(),
                    crate::Timestamp::new(
                        time.duration()
                            + std::time::Duration::from_secs_f32(1.0 / target_framerate as f32),
                    ),
                ));

                let texture = crate::dx::TextureBuilder::new(
                    device,
                    width,
                    height,
                    crate::dx::TextureFormat::NV12,
                )
                .nt_handle()
                .keyed_mutex()
                .build()
                .unwrap();

                crate::dx::copy_texture(&texture, &frame, None)?;

                let sample = make_dxgi_sample(&texture, None)?;

                sample.SetSampleTime(time.hns())?;
                sample.SetSampleDuration(10_000_000 / target_framerate as i64)?;

                // tracing::info!("made sample");

                transform.ProcessInput(input_stream_id, &sample, 0)?;

                // tracing::info!("process input");
            }

            // METransformHaveOutput
            602 => {
                let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();
                output_buffer.dwStatus = 0;
                output_buffer.dwStreamID = output_stream_id;

                let mut output_buffers = [output_buffer];

                let mut status = 0_u32;
                match transform
                    .ProcessOutput(0, &mut output_buffers, &mut status)
                    .map_err(|e| e.code())
                {
                    Ok(_ok) => {
                        let sample = output_buffers[0].pSample.take().unwrap();
                        let media_buffer = unsafe { sample.ConvertToContiguousBuffer() }?;

                        let mut output = vec![];

                        let sample_time;
                        let duration;

                        crate::mf::with_locked_media_buffer(&media_buffer, |data, len| {
                            output.extend_from_slice(&data[..*len]);
                            Ok(())
                        })?;

                        let is_keyframe = match sample.GetUINT32(&MFSampleExtension_CleanPoint) {
                            Ok(1) => crate::FrameIsKeyframe::Yes,
                            Ok(0) => crate::FrameIsKeyframe::No,
                            _ => crate::FrameIsKeyframe::Perhaps,
                        };

                        let sequence_header = if let crate::FrameIsKeyframe::Yes = is_keyframe {
                            // tracing::info!("keyframe!");
                            let output_type = transform.GetOutputCurrentType(output_stream_id)?;
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

                        // tracing::info!("is_keyframe {is_keyframe:?}");

                        // let is_keyframe = sample.GetUINT32(&MFSampleExtension_CleanPoint)? == 1;
                        // let is_keyframe = false;
                        unsafe {
                            sample_time = match sample.GetSampleTime() {
                                Ok(sample_time) => sample_time,
                                Err(_err) => {
                                    // No sample time, but thats fine! we can just throw this sample
                                    tracing::info!("throwing encoder output sample with no sample time attached");
                                    return Ok(());
                                }
                            };

                            duration = std::time::Duration::from_nanos(
                                sample.GetSampleDuration()? as u64 * 100,
                            );
                        };

                        event_tx.blocking_send(EncoderEvent::Data(VideoBuffer {
                            data: output,
                            sequence_header: sequence_header,
                            time: crate::Timestamp::new_hns(sample_time),
                            duration: duration,
                            key_frame: is_keyframe,
                        }))?;
                    }

                    Err(err) => {
                        tracing::info!("encoder err {err}");
                    }
                }
            }

            _ => {
                tracing::warn!("unknown event {event_type}")
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
        crate::dx::TextureBuilder::new(device, width, height, crate::dx::TextureFormat::NV12)
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
            crate::dx::copy_texture(&staging_texture, &frame, None)?;

            // TODO(emily): I see no reason why we shouldn't be able to feed the encoder a texture here.
            // Up above we have an assert that this encoder supports d3d11, so I don't understand why
            // we need to feed it a memory buffer here.

            let media_buffer = MFCreateMemoryBuffer(width * height * 2)?;
            staging_texture.map(context, |texture_data, _source_row_pitch| {
                crate::mf::with_locked_media_buffer(&media_buffer, |data, len| {
                    data.copy_from_slice(texture_data);
                    *len = texture_data.len();
                    Ok(())
                })?;

                Ok(())
            })?;

            let sample = unsafe { MFCreateSample() }?;

            sample.AddBuffer(&media_buffer)?;
            sample.SetSampleTime(time.hns())?;
            sample.SetSampleDuration(10_000_000 / target_framerate as i64)?;

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

                        crate::mf::with_locked_media_buffer(&media_buffer, |data, len| {
                            output.extend_from_slice(&data[..*len]);
                            Ok(())
                        })
                        .unwrap();

                        let is_keyframe = match sample.GetUINT32(&MFSampleExtension_CleanPoint) {
                            Ok(1) => crate::FrameIsKeyframe::Yes,
                            Ok(0) => crate::FrameIsKeyframe::No,
                            _ => crate::FrameIsKeyframe::Perhaps,
                        };

                        unsafe {
                            sample_time = sample.GetUINT64(&MFSampleExtension_DecodeTimestamp)?;

                            duration = std::time::Duration::from_nanos(
                                sample.GetSampleDuration()? as u64 * 100,
                            );

                            sequence_header = if let crate::FrameIsKeyframe::Yes = is_keyframe {
                                // tracing::info!("keyframe!");
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
                                time: crate::Timestamp::new_hns(sample_time as i64),
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
                            tracing::debug!("need more input");
                            break;
                        }
                        Err(MF_E_TRANSFORM_STREAM_CHANGE) => {
                            tracing::warn!("stream change");

                            {
                                for i in 0.. {
                                    if let Ok(output_type) = transform.GetOutputAvailableType(0, i)
                                    {
                                        let subtype = output_type.GetGUID(&MF_MT_SUBTYPE)?;

                                        // super::produce::debug_video_format(&output_type)?;

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
                    tracing::warn!(
                        "encoder is not accepting frames something has gone horribly wrong"
                    )
                }
                Err(err) => todo!("No idea what to do with {err}"),
            }
        }
    }

    eyre::Ok(())
}
