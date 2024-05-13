use std::{
    mem::ManuallyDrop,
    time::{Duration, Instant, UNIX_EPOCH},
};

use eyre::{eyre, Result};
use tokio::sync::mpsc::{self, error::TryRecvError};
use tracing::Instrument;
use windows::{
    core::Interface,
    Win32::{
        Foundation::RECT,
        Graphics::Direct3D11::{
            ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoContext,
            ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
            ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
            D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CAPS,
            D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
            D3D11_VIDEO_PROCESSOR_OUTPUT_RATE_NORMAL, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
            D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
        },
        Media::MediaFoundation::*,
    },
};

use crate::{
    dx::{self, ID3D11Texture2DExt},
    media_queue::MediaQueue,
    statistics::ConversionStatistics,
    texture_pool::{Texture, TexturePool},
    ARBITRARY_MEDIA_CHANNEL_LIMIT,
};

use super::{
    dx::copy_texture,
    mf::{self, make_dxgi_sample, IMFAttributesExt, IMFDXGIBufferExt},
};

pub(crate) enum ConvertControl {
    Frame(Texture, crate::Timestamp),
}
pub(crate) enum ConvertEvent {
    Frame(Texture, crate::Timestamp, ConversionStatistics),
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

#[tracing::instrument]
pub(crate) async fn converter(
    output_width: u32,
    output_height: u32,
    input_format: Format,
    output_format: Format,
) -> Result<(mpsc::Sender<ConvertControl>, mpsc::Receiver<ConvertEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "color-converter-control").await;
    telemetry::client::watch_channel(&event_tx, "color-converter-event").await;

    let span = tracing::Span::current();

    tokio::task::spawn_blocking(move || {
        unsafe {
            let span_guard = span.enter();

            tracing::debug!("starting");

            scopeguard::defer! {
                tracing::debug!("stopping");
            }

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

            // TODO(emily): We make the texture pool here twice on startup
            // NOTE(emily): cc requires that the input texture be bind_render_target and bind_shader_resource
            let input_texture_pool = TexturePool::new(
                || {
                    super::dx::TextureBuilder::new(
                        &device,
                        output_width,
                        output_height,
                        input_format.into(),
                    )
                    .bind_render_target()
                    .bind_shader_resource()
                    .build()
                    .unwrap()
                },
                10,
            );

            let output_texture_pool = TexturePool::new(
                || {
                    super::dx::TextureBuilder::new(
                        &device,
                        output_width,
                        output_height,
                        output_format.into(),
                    )
                    .keyed_mutex()
                    .nt_handle()
                    .build()
                    .unwrap()
                },
                10,
            );

            let mut media_queue = MediaQueue::new();

            let set_format_types = |input_width, input_height| -> Result<()> {
                {
                    let input_type = MFCreateMediaType()?;

                    input_type.set_guid(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    input_type.set_guid(&MF_MT_SUBTYPE, &input_format.into())?;

                    input_type.set_fraction(&MF_MT_FRAME_SIZE, input_width, input_height)?;
                    input_type.set_u32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;

                    mf::debug_video_format(&input_type)?;

                    transform.SetInputType(0, &input_type, 0)?;
                }

                for i in 0.. {
                    let output_type = transform.GetOutputAvailableType(0, i)?;

                    let format = output_type.get_guid(&MF_MT_SUBTYPE)?;

                    output_type.set_fraction(&MF_MT_FRAME_SIZE, output_width, output_height)?;

                    if format == output_format.into() {
                        mf::debug_video_format(&output_type)?;
                        transform.SetOutputType(0, &output_type, 0)?;

                        break;
                    }
                }

                input_texture_pool.update_texture_format(
                    || {
                        super::dx::TextureBuilder::new(
                            &device,
                            input_width,
                            input_height,
                            input_format.into(),
                        )
                        .bind_render_target()
                        .bind_shader_resource()
                        .build()
                        .unwrap()
                    },
                    10,
                );

                Ok(())
            };

            set_format_types(output_width, output_height)?;

            let (mut last_input_width, mut last_input_height) = (output_width, output_height);

            loop {
                // TODO(emily): Like in encoder why are we pulling multiple frames here, just pull the current one
                let ConvertControl::Frame(frame, timestamp) = {
                    let mut control = None;

                    loop {
                        match control_rx.try_recv() {
                            Ok(convert_control) => control = Some(convert_control),
                            Err(TryRecvError::Disconnected) => {
                                tracing::debug!("control is gone");
                                return Ok(());
                            }
                            Err(TryRecvError::Empty) => break,
                        }
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
                let start_time = Instant::now();

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

                        media_queue.drain();

                        // Set the input type
                        set_format_types(width, height)?;
                        // Update our last types
                        (last_input_width, last_input_height) = (width, height);
                    }

                    let texture = input_texture_pool.acquire();

                    super::dx::copy_texture(&texture, &frame, None)?;

                    media_queue.push_back(texture.clone());

                    make_dxgi_sample(&texture, None)?
                };

                sample.SetSampleTime(10)?;
                // sample.SetSampleDuration(10_000_000 / target_framerate as i64)?;
                sample.SetSampleDuration(1000)?;

                let result = transform
                    .ProcessInput(0, &sample, 0)
                    .map_err(|err| err.code());

                tracing::trace!("process input {result:?}");

                match result {
                    Ok(_) | Err(MF_E_NOTACCEPTING) => {
                        let output_result = process_output(&transform, &output_texture_pool)
                            .map_err(|err| err.code());

                        match output_result {
                            Ok((output_texture, _)) => {
                                media_queue.pop_front();

                                match event_tx.blocking_send(ConvertEvent::Frame(
                                    output_texture,
                                    timestamp,
                                    ConversionStatistics {
                                        media_queue_len: 0,
                                        time: start_time.elapsed(),
                                    },
                                )) {
                                    Err(err) => {
                                        tracing::debug!("Failed to send convert event, done");
                                        break;
                                    }
                                    Ok(()) => {}
                                }
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
        }
    });

    Ok((control_tx, event_rx))
}

fn process_output(
    transform: &IMFTransform,
    texture_pool: &TexturePool,
) -> Result<(Texture, crate::Timestamp), windows::core::Error> {
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

                let output_texture = texture_pool.acquire();

                copy_texture(&output_texture, &texture, Some(subresource_index))?;

                Ok((output_texture, crate::Timestamp::new_hns(timestamp_hns)))
            }
            Err(err) => Err(err),
        }
    }
}

#[tracing::instrument]
pub(crate) async fn dxva_converter(
    output_width: u32,
    output_height: u32,
    input_format: Format,
    output_format: Format,
) -> Result<(mpsc::Sender<ConvertControl>, mpsc::Receiver<ConvertEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);

    let span = tracing::Span::current();
    tokio::task::spawn_blocking(move || unsafe {
        let _span_guard = span.enter();

        let (device, context) = super::dx::create_device()?;

        struct Storage {
            input_width: u32,
            input_height: u32,
            processor: ID3D11VideoProcessor,
            context: ID3D11VideoContext,
            input_texture: ID3D11Texture2D,
            output_texture: ID3D11Texture2D,
            input_view: ID3D11VideoProcessorInputView,
            output_view: ID3D11VideoProcessorOutputView,
            output_texture_pool: TexturePool,
        }

        unsafe fn build_storage(
            input_width: u32,
            input_height: u32,
            output_width: u32,
            output_height: u32,
            input_format: Format,
            output_format: Format,
            device: &ID3D11Device,
            context: &ID3D11DeviceContext,
        ) -> Result<Storage> {
            let video_device = device.cast::<ID3D11VideoDevice>()?;

            let content_description = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputWidth: input_width,
                InputHeight: input_height,
                OutputWidth: output_width,
                OutputHeight: output_height,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
                ..Default::default()
            };

            let enumerator = video_device.CreateVideoProcessorEnumerator(&content_description)?;

            let mut caps = D3D11_VIDEO_PROCESSOR_CAPS::default();
            enumerator.GetVideoProcessorCaps(&mut caps)?;

            let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;
            let context = context.cast::<ID3D11VideoContext>()?;

            context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );

            context.VideoProcessorSetStreamOutputRate(
                &processor,
                0,
                D3D11_VIDEO_PROCESSOR_OUTPUT_RATE_NORMAL,
                true,
                None,
            );

            let input_rect = RECT {
                left: 0,
                top: 0,
                right: input_width as i32,
                bottom: input_height as i32,
            };

            let output_rect = RECT {
                left: 0,
                top: 0,
                right: output_width as i32,
                bottom: output_height as i32,
            };

            context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&input_rect));
            context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&output_rect));
            context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&output_rect));

            let input_texture = crate::dx::TextureBuilder::new(
                &device,
                input_width,
                input_height,
                input_format.into(),
            )
            .build()
            .unwrap();

            let mut input_view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC::default();
            input_view_desc.FourCC = 0;
            input_view_desc.ViewDimension = D3D11_VPIV_DIMENSION_TEXTURE2D;

            let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
            video_device.CreateVideoProcessorInputView(
                &input_texture,
                &enumerator,
                &input_view_desc,
                Some(&mut input_view),
            )?;

            let input_view = input_view.unwrap();

            let output_texture_pool = TexturePool::new(
                || {
                    crate::dx::TextureBuilder::new(
                        &device,
                        output_width,
                        output_height,
                        output_format.into(),
                    )
                    .keyed_mutex()
                    .nt_handle()
                    .build()
                    .unwrap()
                },
                10,
            );

            let output_texture = crate::dx::TextureBuilder::new(
                &device,
                output_width,
                output_height,
                output_format.into(),
            )
            .bind_render_target()
            .build()
            .unwrap();

            let mut output_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC::default();
            output_view_desc.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;

            let mut output_view: Option<ID3D11VideoProcessorOutputView> = None;
            video_device.CreateVideoProcessorOutputView(
                &output_texture,
                &enumerator,
                &output_view_desc,
                Some(&mut output_view),
            )?;

            let output_view = output_view.unwrap();

            Ok(Storage {
                context,
                input_height,
                input_width,
                input_texture,
                output_texture,
                input_view,
                output_texture_pool,
                output_view,
                processor,
            })
        }

        // NOTE(emily): Initally assume that the input and output are the same dimensions
        let mut storage = build_storage(
            output_width,
            output_height,
            output_width,
            output_height,
            input_format,
            output_format,
            &device,
            &context,
        )?;

        loop {
            // TODO(emily): Like in encoder why are we pulling multiple frames here, just pull the current one
            let ConvertControl::Frame(frame, timestamp) = {
                let mut control = None;

                loop {
                    match control_rx.try_recv() {
                        Ok(convert_control) => control = Some(convert_control),
                        Err(TryRecvError::Disconnected) => {
                            tracing::debug!("control is gone");
                            return Ok(());
                        }
                        Err(TryRecvError::Empty) => break,
                    }
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
            let start_time = Instant::now();

            let desc = frame.desc();

            if desc.Width != storage.input_width || desc.Height != storage.input_height {
                storage = build_storage(
                    desc.Width,
                    desc.Height,
                    output_width,
                    output_height,
                    input_format,
                    output_format,
                    &device,
                    &context,
                )?;
            }

            dx::copy_texture(&storage.input_texture, &frame, None)?;

            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(storage.input_view.clone())),
                ..Default::default()
            };

            storage.context.VideoProcessorBlt(
                &storage.processor,
                Some(&storage.output_view),
                0,
                &[stream],
            )?;

            let output = storage.output_texture_pool.acquire();

            dx::copy_texture(&output, &storage.output_texture, None)?;

            event_tx.blocking_send(ConvertEvent::Frame(
                output,
                timestamp,
                ConversionStatistics {
                    media_queue_len: 0,
                    time: Instant::now() - start_time,
                },
            ))?;
        }

        eyre::Ok(())
    });

    Ok((control_tx, event_rx))
}
