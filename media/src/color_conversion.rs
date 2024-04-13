use std::{
    time::UNIX_EPOCH,
};

use eyre::{eyre, Result};
use tokio::sync::mpsc;
use windows::{
    core::ComInterface,
    Win32::{
        Graphics::Direct3D11::ID3D11Texture2D,
        Media::MediaFoundation::*,
    },
};

use crate::ARBITRARY_CHANNEL_LIMIT;

use super::{
    dx::copy_texture,
    mf::{self, make_dxgi_sample, IMFAttributesExt, IMFDXGIBufferExt},
};

pub(crate) enum ConvertControl {
    Frame(ID3D11Texture2D, crate::Timestamp),
}
pub(crate) enum ConvertEvent {
    Frame(ID3D11Texture2D, crate::Timestamp),
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
            Format::BGRA => MFVideoFormat_ARGB32,
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
    target_framerate: u32,
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
                mf::init()?;

                let (device, _context) = super::dx::create_device()?;

                let device_manager = super::mf::create_dxgi_manager(&device)?;

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

                if attributes.get_u32(&MF_SA_D3D11_AWARE)? != 1 {
                    panic!("Not D3D11 aware");
                }

                transform.ProcessMessage(
                    MFT_MESSAGE_SET_D3D_MANAGER,
                    std::mem::transmute(device_manager),
                )?;

                attributes.set_u32(&MF_LOW_LATENCY, 1)?;


                {
                    let input_type = MFCreateMediaType()?;

                    input_type.set_guid(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    input_type.set_guid(&MF_MT_SUBTYPE, &input_format.into())?;

                    input_type.set_fraction(&MF_MT_FRAME_SIZE, width, height)?;
                    input_type.set_u32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;

                    transform.SetInputType(0, &input_type, 0)?;
                }

                for i in 0.. {
                    let output_type = transform.GetOutputAvailableType(0, i)?;

                    let format = output_type.get_guid(&MF_MT_SUBTYPE)?;

                    output_type.set_fraction(&MF_MT_FRAME_SIZE, width, height)?;

                    if format == output_format.into() {
                        transform.SetOutputType(0, &output_type, 0)?;
                        break;
                    }
                }

                loop {
                    let ConvertControl::Frame(frame, time) = {
                        let mut control = None;
    
                        while let Ok(new_control) = control_rx.try_recv() {
                            control = Some(new_control);
                        }
    
                        // If we didn't get a frame then wait for one now
                        if control.is_none() {
                            control = Some(
                                control_rx
                                    .blocking_recv()
                                    .ok_or(eyre!("cc control closed"))?,
                            );
                            // match control_rx.blocking_recv() {
                            //     Some(control) => Some(control),
                            //     None => return Err(eyre::eyre!("encoder control closed")),
                            // };
                        };

                        control.unwrap()
                    };

                    // NOTE(emily): If this frame elapsed then throw it before trying to make a texture,
                    // do this after getting the sample from the media resource
                    // if let Ok(d) = time.elapsed() {
                    //     if d > std::time::Duration::from_millis(10) {
                    //         tracing::info!("cc throwing expired frame (before input) {}ms", d.as_millis());
                    //         continue;
                    //     }
                    // }

                    let texture = super::dx::TextureBuilder::new(&device, width, height, input_format.into()).bind_render_target().bind_shader_resource().build()?;

                    super::dx::copy_texture(&texture, &frame, None)?;

                    let sample = make_dxgi_sample(&texture, None)?;

                    sample
                        .SetSampleTime(time.hns())?;
                    sample.SetSampleDuration(10_000_000 / target_framerate as i64)?;

                    let process_output = || -> Result<Option<(ID3D11Texture2D, crate::Timestamp)>, windows::core::Error> {
                        let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();
                        output_buffer.dwStatus = 0;
                        output_buffer.dwStreamID = 0;

                        let mut output_buffers = [output_buffer];

                        let mut status = 0_u32;
                        match transform.ProcessOutput(0, &mut output_buffers, &mut status) {
                            Ok(_ok) => {
                                let sample = output_buffers[0].pSample.take().unwrap();

                                let timestamp_hns = unsafe { sample.GetSampleTime()? };

                                let media_buffer = unsafe { sample.GetBufferByIndex(0) }?;
                                let dxgi_buffer: IMFDXGIBuffer = media_buffer.cast()?;

                                let (texture, subresource_index) = dxgi_buffer.texture()?;

                                // NOTE(emily): If this frame elapsed then throw it before trying to make a texture,
                                // do this after getting the sample from the media resource
                                // if let Ok(d) = time.elapsed() {
                                //     if d > std::time::Duration::from_millis(10) {
                                //         tracing::info!("cc throwing expired frame {}ms", d.as_millis());
                                //         return Ok(None);
                                //     }
                                // }

                                let output_texture = super::dx::TextureBuilder::new(
                                    &device,
                                    width,
                                    height,
                                    output_format.into(),
                                )
                                .keyed_mutex()
                                .nt_handle()
                                .build()
                                .unwrap();

                                copy_texture(&output_texture, &texture, Some(subresource_index))?;

                                Ok(Some((output_texture, crate::Timestamp::new_hns(timestamp_hns))))
                            }
                            Err(err) => {
                                // tracing::warn!("output flags {}", output_buffers[0].dwStatus);
                                Err(err)
                            }
                        }
                    };

                    let result = transform
                        .ProcessInput(0, &sample, 0)
                        .map_err(|err| err.code());

                    tracing::trace!("cc process input {result:?}");

                    match result
                    {
                        Ok(_) | Err(MF_E_NOTACCEPTING) => {
                            let output_result = process_output().map_err(|err| err.code());

                            tracing::trace!("cc process output {output_result:?}");

                            match output_result {
                                Ok(Some((output_texture, timestamp))) => {
                                    match event_tx.try_send(ConvertEvent::Frame(output_texture.clone(), timestamp)) {
                                        Ok(_) => {},
                                        Err(err) => match err {
                                            mpsc::error::TrySendError::Full(_) => tracing::info!("cc backpressured"),
                                            mpsc::error::TrySendError::Closed(_) => { tracing::warn!("cc event closed"); return Err(eyre!("cc event closed")); },
                                        },
                                    }
                                }
                                Ok(None) => {
                                    // Continue trying to get more frames
                                    tracing::trace!("cc trying to get more frames")
                                }
                                Err(MF_E_TRANSFORM_NEED_MORE_INPUT) => {
                                    tracing::trace!("cc needs more input");
                                }
                                Err(MF_E_TRANSFORM_STREAM_CHANGE) => {
                                    unreachable!("Not expecting a stream format change");
                                }
                                Err(err) => {
                                    // tracing::error!("No idea what to do with {err}");
                                    // break;
                                    todo!("No idea what to do with {err}")
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
                Ok(_) => tracing::warn!("cc exit Ok"),
                Err(err) => tracing::error!("cc exit err {err} {err:?}"),
            }
        }
    });

    Ok((control_tx, event_rx))
}
