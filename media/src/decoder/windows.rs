

use ::windows::{
    core::ComInterface,
    Win32::{
        Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext},
        Media::MediaFoundation::*,
    },
};
use eyre::Result;
use tokio::sync::mpsc;

use crate::{VideoBuffer, ARBITRARY_CHANNEL_LIMIT};

use crate::{
    dx::{copy_texture, MapTextureExt, TextureCPUAccess, TextureUsage},
    mf::{self, IMFAttributesExt, IMFDXGIBufferExt},
};

use super::{DecoderControl, DecoderEvent};

pub async fn h264_decoder(
    width: u32,
    height: u32,
    target_framerate: u32,
    target_bitrate: u32,
) -> Result<(mpsc::Sender<DecoderControl>, mpsc::Receiver<DecoderEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "h264-decoder-control").await;
    telemetry::client::watch_channel(&event_tx, "h264-decoder-event").await;

    tokio::spawn({
        async move {
            match tokio::task::spawn_blocking(move || unsafe {
                mf::init()?;

                let (device, context) = crate::dx::create_device()?;

                let device_manager = crate::mf::create_dxgi_manager(&device)?;

                let find_decoder = |hardware| {
                    let mut count = 0_u32;
                    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();

                    MFTEnumEx(
                        MFT_CATEGORY_VIDEO_DECODER,
                        if hardware {
                            MFT_ENUM_FLAG_SYNCMFT
                        } else {
                            MFT_ENUM_FLAG(0)
                        } | MFT_ENUM_FLAG_SORTANDFILTER,
                        Some(&MFT_REGISTER_TYPE_INFO {
                            guidMajorType: MFMediaType_Video,
                            guidSubtype: MFVideoFormat_H264_ES,
                        }),
                        None,
                        &mut activates,
                        &mut count,
                    )?;

                    // TODO(emily): CoTaskMemFree activates

                    let activates = std::slice::from_raw_parts_mut(activates, count as usize);
                    let activate = activates.first().ok_or_else(|| eyre::eyre!("No decoders"))?;

                    // NOTE(emily): If there is an activate then it should be real
                    let activate = activate.as_ref().unwrap();

                    let transform: IMFTransform = activate.ActivateObject()?;

                    eyre::Ok(transform)
                };

                let transform = match find_decoder(true) {
                    Ok(decoder) => decoder,
                    Err(err) => {
                        log::warn!("unable to find a hardware h264 decoder {err}, falling back to a software encoder");
                        find_decoder(false)?
                    }
                };

                let attributes = transform.GetAttributes()?;

                attributes.set_u32(&CODECAPI_AVLowLatencyMode, 1)?;
                attributes.set_u32(&CODECAPI_AVDecNumWorkerThreads, 8)?;
                attributes.set_u32(&CODECAPI_AVDecVideoAcceleration_H264, 1)?;
                attributes.set_u32(&CODECAPI_AVDecVideoThumbnailGenerationMode, 0)?;

                // TODO(emily): NOTE from MSDN:
                // This attribute applies only to video MFTs. To query this attribute, call IMFTransform::GetAttributes 
                // to get the MFT attribute store. If GetAttributes succeeds, call IMFAttributes::GetUINT32.

                // * If the attribute is nonzero, the client can give the MFT a pointer to the IMFDXGIDeviceManager 
                //   interface before streaming starts. To do so, the client sends the MFT_MESSAGE_SET_D3D_MANAGER 
                //   message to the MFT. The client is not required to send this message.
                // * If this attribute is zero (FALSE), the MFT does not support Direct3D 11, and the client should not 
                //   send the MFT_MESSAGE_SET_D3D_MANAGER message to the MFT.

                // The default value of this attribute is FALSE. Treat this attribute as read-only. 
                // Do not change the value; the MFT will ignore any changes to the value.

                // NOTE(emily): What I don't understand here is that even in VM on apple M1, we pass the D3D11 aware
                // check but we cannot set a d3d manager. This is in complete contrast to what the MSDN article above
                // suggests.

                if attributes.get_u32(&MF_SA_D3D11_AWARE)? != 1 {
                    panic!("Not D3D11 aware");
                }

                // NOTE(emily): If this fails then this is a sofware encoder
                let is_hardware_transform = transform.ProcessMessage(
                    MFT_MESSAGE_SET_D3D_MANAGER,
                    std::mem::transmute(device_manager),
                ).map(|_| true).unwrap_or_default();

                attributes.set_u32(&MF_LOW_LATENCY, 1)?;

                {
                    let input_type = transform.GetInputAvailableType(0, 0)?;

                    input_type.set_guid(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    input_type.set_guid(&MF_MT_SUBTYPE, &MFVideoFormat_H264_ES)?;

                    input_type.set_fraction(&MF_MT_FRAME_SIZE, width, height)?;
                    input_type.set_fraction(&MF_MT_FRAME_RATE, target_framerate, 1)?;
                    input_type.set_fraction(&MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;

                    input_type.set_u32(&MF_MT_AVG_BITRATE, target_bitrate)?;

                    input_type
                        .set_u32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;

                    transform.SetInputType(0, &input_type, 0)?;
                }

                {
                    let output_type = transform.GetOutputAvailableType(0, 0)?;
                    output_type.set_guid(
                        &MF_MT_MAJOR_TYPE,
                        &MFMediaType_Video,
                    )?;
                    output_type
                        .set_guid(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;

                    output_type.set_u32(&MF_MT_AVG_BITRATE, target_bitrate)?;

                    output_type.set_fraction(&MF_MT_FRAME_SIZE, width, height)?;
                    output_type.set_fraction(&MF_MT_FRAME_RATE, target_framerate, 1)?;
                    output_type.set_fraction(&MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;

                    transform.SetOutputType(0, &output_type, 0)?;
                }


                if is_hardware_transform {
                    hardware(control_rx, transform, device, width, height, event_tx)?
                } else {
                    software(&device, &context, control_rx, transform, width, height, event_tx)?;
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

unsafe fn hardware(
    mut control_rx: mpsc::Receiver<DecoderControl>,
    transform: IMFTransform,
    device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    width: u32,
    height: u32,
    event_tx: mpsc::Sender<DecoderEvent>,
) -> Result<()> {
    transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
    transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
    transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

    loop {
        let DecoderControl::Data(VideoBuffer {
            data,
            sequence_header,
            time,
            duration,
            key_frame: _,
        }) = control_rx
            .blocking_recv()
            .ok_or(eyre::eyre!("decoder control closed"))?;

        if let Some(sequence_header) = sequence_header {
            let input_type = transform.GetInputCurrentType(0)?;
            input_type.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &sequence_header)?;
        }

        let len = data.len();

        let media_buffer = MFCreateMemoryBuffer(len as u32)?;

        crate::mf::with_locked_media_buffer(&media_buffer, |buffer, len| {
            buffer.copy_from_slice(&data);
            *len = buffer.len();
            Ok(())
        })?;

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
            let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();
            output_buffer.dwStatus = 0;
            output_buffer.dwStreamID = 0;

            let mut output_buffers = [output_buffer];

            let mut status = 0_u32;
            match transform.ProcessOutput(0, &mut output_buffers, &mut status) {
                Ok(ok) => {
                    let output_texture = crate::dx::TextureBuilder::new(
                        &device,
                        width,
                        height,
                        crate::dx::TextureFormat::NV12,
                    )
                    .keyed_mutex()
                    .nt_handle()
                    .build()
                    .unwrap();

                    let sample = output_buffers[0].pSample.take().unwrap();
                    let timestamp = unsafe { sample.GetSampleTime()? };

                    let media_buffer = unsafe { sample.GetBufferByIndex(0) }?;
                    let dxgi_buffer: IMFDXGIBuffer = media_buffer.cast()?;

                    let (texture, subresource_index) = dxgi_buffer.texture()?;

                    copy_texture(&output_texture, &texture, Some(subresource_index))?;

                    event_tx
                        .blocking_send(DecoderEvent::Frame(
                            output_texture,
                            std::time::SystemTime::UNIX_EPOCH
                                + std::time::Duration::from_nanos(timestamp as u64 * 100),
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
                                if let Ok(output_type) = transform.GetOutputAvailableType(0, i) {
                                    let subtype = output_type.get_guid(&MF_MT_SUBTYPE)?;

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

                        // transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
                    }
                    Err(err) => todo!("No idea what to do with {err}"),
                };
            },
            Err(MF_E_NOTACCEPTING) => {
                log::warn!("decoder is not accepting frames something has gone horribly wrong")
            }
            Err(err) => todo!("No idea what to do with {err}"),
        }
    }

    Ok(())
}

unsafe fn software(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    mut control_rx: mpsc::Receiver<DecoderControl>,
    transform: IMFTransform,
    width: u32,
    height: u32,
    event_tx: mpsc::Sender<DecoderEvent>,
) -> Result<()> {
    let staging_texture =
        crate::dx::TextureBuilder::new(device, width, height, crate::dx::TextureFormat::NV12)
            .usage(TextureUsage::Staging)
            .cpu_access(TextureCPUAccess::Read)
            .build()?;

    transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
    transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

    loop {
        let DecoderControl::Data(VideoBuffer {
            data,
            sequence_header,
            time,
            duration,
            key_frame: _,
        }) = control_rx
            .blocking_recv()
            .ok_or(eyre::eyre!("decoder control closed"))?;

        if let Some(sequence_header) = sequence_header {
            let input_type = transform.GetInputCurrentType(0)?;
            input_type.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &sequence_header)?;
        }

        let len = data.len();

        let media_buffer = MFCreateMemoryBuffer(len as u32)?;

        crate::mf::with_locked_media_buffer(&media_buffer, |buffer, len| {
            buffer.copy_from_slice(&data);
            *len = buffer.len();
            Ok(())
        })?;

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
            let output_stream_info = transform.GetOutputStreamInfo(0)?;

            let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();

            let output_sample = MFCreateSample()?;
            let media_buffer = MFCreateMemoryBuffer(output_stream_info.cbSize)?;
            output_sample.AddBuffer(&media_buffer)?;

            output_buffer.pSample = std::mem::ManuallyDrop::new(Some(output_sample));
            output_buffer.dwStatus = 0;
            output_buffer.dwStreamID = 0;

            let mut output_buffers = [output_buffer];

            let mut status = 0_u32;
            match transform.ProcessOutput(0, &mut output_buffers, &mut status) {
                Ok(ok) => {
                    let output_texture = crate::dx::TextureBuilder::new(
                        &device,
                        width,
                        height,
                        crate::dx::TextureFormat::NV12,
                    )
                    .keyed_mutex()
                    .build()
                    .unwrap();

                    let sample = output_buffers[0].pSample.take().unwrap();
                    let timestamp = unsafe { sample.GetSampleTime()? };

                    let media_buffer = unsafe { sample.ConvertToContiguousBuffer() }?;

                    staging_texture
                        .map_mut(context, |texture_data, _stride| {
                            crate::mf::with_locked_media_buffer(&media_buffer, |buffer, _len| {
                                texture_data.copy_from_slice(buffer);
                                Ok(())
                            })
                        })
                        .unwrap();

                    copy_texture(&output_texture, &staging_texture, None)?;

                    event_tx
                        .blocking_send(DecoderEvent::Frame(
                            output_texture,
                            std::time::SystemTime::UNIX_EPOCH
                                + std::time::Duration::from_nanos(timestamp as u64 * 100),
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
                                if let Ok(output_type) = transform.GetOutputAvailableType(0, i) {
                                    let subtype = output_type.GetGUID(&MF_MT_SUBTYPE)?;

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
                log::warn!("decoder is not accepting frames something has gone horribly wrong")
            }
            Err(err) => todo!("No idea what to do with {err}"),
        }
    }

    Ok(())
}
