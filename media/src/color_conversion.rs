use std::time::UNIX_EPOCH;

use eyre::{eyre, Result};
use tokio::sync::mpsc;
use windows::{
    core::Interface,
    Win32::{
        Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D},
        Media::MediaFoundation::*,
    },
};

use crate::{dx::ID3D11Texture2DExt, ARBITRARY_CHANNEL_LIMIT};

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

#[derive(Debug, Copy, Clone)]
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

// #[tracing::instrument]
pub(crate) async fn converter(
    output_width: u32,
    output_height: u32,
    target_framerate: u32,
    input_format: Format,
    output_format: Format,
) -> Result<(mpsc::Sender<ConvertControl>, mpsc::Receiver<ConvertEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "color-converter-control").await;
    telemetry::client::watch_channel(&event_tx, "color-converter-event").await;

    let span = tracing::Span::current();

    tokio::spawn({
        async move {
            match tokio::task::spawn_blocking(move || unsafe {
                let _guard = span.enter();

                tracing::debug!("starting");

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

                let set_format_types = |input_width, input_height| -> Result<()> {
                    {
                        let input_type = MFCreateMediaType()?;

                        input_type.set_guid(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                        input_type.set_guid(&MF_MT_SUBTYPE, &input_format.into())?;

                        input_type.set_fraction(&MF_MT_FRAME_SIZE, input_width, input_height)?;
                        input_type.set_u32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;

                        transform.SetInputType(0, &input_type, 0)?;
                    }

                    for i in 0.. {
                        let output_type = transform.GetOutputAvailableType(0, i)?;

                        let format = output_type.get_guid(&MF_MT_SUBTYPE)?;

                        output_type.set_fraction(&MF_MT_FRAME_SIZE, output_width, output_height)?;

                        if format == output_format.into() {
                            transform.SetOutputType(0, &output_type, 0)?;
                            break;
                        }
                    }

                    Ok(())
                };

                set_format_types(output_width, output_height)?;

                let (mut last_input_width, mut last_input_height) = (output_width, output_height);

                loop {
                    let ConvertControl::Frame(frame, time) = {
                        let mut control = None;

                        // TODO(emily): Properly handle TryRecvErr::Disconnected.
                        // Right now we will never exit when we see that, I think that is probably
                        // wrong.
                        while let Ok(new_control) = control_rx.try_recv() {
                            control = Some(new_control);
                        }

                        // If we didn't get a frame then wait for one now
                        if control.is_none() {
                            control = Some(
                                control_rx
                                    .blocking_recv()
                                    .ok_or(eyre!("ConvertControl channel closed"))?,
                            );
                        };

                        control.unwrap()
                    };

                    // NOTE(emily): cc requires that the input texture be bind_render_target and bind_shader_resource

                    let sample = {
                        let (width, height) = {
                            let desc = frame.desc();
                            (desc.Width, desc.Height)
                        };

                        if width != last_input_width || height != last_input_height {
                            tracing::debug!(
                                width,
                                height,
                                last_input_width,
                                last_input_height,
                                "input changed"
                            );
                            // Change input type according to
                            // https://learn.microsoft.com/en-us/windows/win32/medfound/handling-stream-changes
                            // drain mft
                            transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
                            let mut status = 0;
                            while let Err(MF_E_TRANSFORM_NEED_MORE_INPUT) = transform
                                .ProcessOutput(0, &mut [], &mut status)
                                .map_err(|e| e.code())
                            {}

                            // Set the input type
                            set_format_types(width, height)?;
                            // Update our last types
                            (last_input_width, last_input_height) = (width, height);
                        }

                        let texture = super::dx::TextureBuilder::new(
                            &device,
                            width,
                            height,
                            input_format.into(),
                        )
                        .bind_render_target()
                        .bind_shader_resource()
                        .build()?;

                        super::dx::copy_texture(&texture, &frame, None)?;
                        make_dxgi_sample(&texture, None)?
                    };

                    sample.SetSampleTime(time.hns())?;
                    sample.SetSampleDuration(10_000_000 / target_framerate as i64)?;

                    let result = transform
                        .ProcessInput(0, &sample, 0)
                        .map_err(|err| err.code());

                    tracing::trace!("cc process input {result:?}");

                    match result {
                        Ok(_) | Err(MF_E_NOTACCEPTING) => {
                            let output_result = process_output(
                                &transform,
                                &device,
                                output_width,
                                output_height,
                                output_format,
                            )
                            .map_err(|err| err.code());

                            tracing::trace!("cc process output {output_result:?}");

                            match output_result {
                                Ok(Some((output_texture, timestamp))) => {
                                    match event_tx.try_send(ConvertEvent::Frame(
                                        output_texture.clone(),
                                        timestamp,
                                    )) {
                                        Ok(_) => {}
                                        Err(err) => match err {
                                            mpsc::error::TrySendError::Full(_) => {
                                                tracing::info!("backpressured")
                                            }
                                            mpsc::error::TrySendError::Closed(_) => {
                                                tracing::info!("event closed");
                                                return Err(eyre!("event closed"));
                                            }
                                        },
                                    }
                                }
                                Ok(None) => {
                                    // Continue trying to get more frames
                                    tracing::trace!("trying to get more frames")
                                }
                                Err(MF_E_TRANSFORM_NEED_MORE_INPUT) => {
                                    tracing::trace!("needs more input");
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
                        }
                        Err(err) => todo!("No idea what to do with {err}"),
                    }
                }

                eyre::Ok(())
            })
            .await
            .unwrap()
            {
                Ok(_) => tracing::info!("exit Ok"),
                Err(err) => tracing::error!("exit err {err} {err:?}"),
            }
        }
    });

    Ok((control_tx, event_rx))
}

fn process_output(
    transform: &IMFTransform,
    device: &ID3D11Device,
    output_width: u32,
    output_height: u32,
    output_format: Format,
) -> Result<Option<(ID3D11Texture2D, crate::Timestamp)>, windows::core::Error> {
    unsafe {
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

                let output_texture = super::dx::TextureBuilder::new(
                    &device,
                    output_width,
                    output_height,
                    output_format.into(),
                )
                .keyed_mutex()
                .nt_handle()
                .build()
                .unwrap();

                copy_texture(&output_texture, &texture, Some(subresource_index))?;

                Ok(Some((
                    output_texture,
                    crate::Timestamp::new_hns(timestamp_hns),
                )))
            }
            Err(err) => Err(err),
        }
    }
}
