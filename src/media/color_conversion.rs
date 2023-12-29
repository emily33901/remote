use std::{
    mem::{ManuallyDrop, MaybeUninit},
    time::UNIX_EPOCH,
};

use eyre::Result;
use tokio::sync::{
    mpsc,
    mpsc::error::{TryRecvError, TrySendError},
};
use windows::{
    core::{s, ComInterface, PWSTR},
    Win32::{
        Foundation::FALSE,
        Foundation::S_OK,
        Graphics::{
            Direct3D::{D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D11_SRV_DIMENSION_TEXTURE2D},
            Direct3D11::{
                ID3D11Buffer, ID3D11InputLayout, ID3D11PixelShader, ID3D11RenderTargetView,
                ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
                D3D11_APPEND_ALIGNED_ELEMENT, D3D11_BIND_VERTEX_BUFFER, D3D11_BOX,
                D3D11_BUFFER_DESC, D3D11_COMPARISON_NEVER, D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA, D3D11_RESOURCE_MISC_FLAG,
                D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_SAMPLER_DESC,
                D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SUBRESOURCE_DATA, D3D11_TEX2D_SRV,
                D3D11_TEXTURE2D_DESC, D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT,
                D3D11_VIEWPORT,
            },
            Dxgi::{
                Common::{
                    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_FORMAT_R32G32_FLOAT,
                    DXGI_FORMAT_R8G8_UNORM, DXGI_FORMAT_R8_UNORM,
                },
                IDXGIKeyedMutex,
            },
        },
        Media::MediaFoundation::*,
        System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE},
    },
};

use crate::{media::dx::compile_shader, video::VideoBuffer, ARBITRARY_CHANNEL_LIMIT};

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

                // Some(&MFT_REGISTER_TYPE_INFO {
                //         guidMajorType: MFMediaType_Video,
                //         guidSubtype: input_format.into(),
                //     }),
                //     Some(&MFT_REGISTER_TYPE_INFO {
                //         guidMajorType: MFMediaType_Video,
                //         guidSubtype: output_format.into(),
                //     })

                MFTEnumEx(
                    MFT_CATEGORY_VIDEO_PROCESSOR,
                    MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                    None,
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

                let pvc: IMFVideoProcessorControl3 = transform.cast()?;

                pvc.EnableHardwareEffects(true)?;

                // attributes.SetUINT32(&MF_LOW_LATENCY as *const _, 1)?;

                {
                    let input_type = MFCreateMediaType()?;
                    // let input_type = transform.GetInputAvailableType(0, 0)?;

                    input_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    input_type.SetGUID(&MF_MT_SUBTYPE, &input_format.into())?;

                    let width_height = (width as u64) << 32 | (height as u64);
                    input_type.SetUINT64(&MF_MT_FRAME_SIZE, width_height)?;

                    input_type.SetUINT64(&MF_MT_FRAME_RATE, (5 << 32) | (1))?;

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

                            output_type.SetUINT64(&MF_MT_FRAME_RATE, (5 << 32) | (1))?;

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

                let output_sample = MFCreateVideoSampleFromSurface(&output_sample_texture)?;

                loop {
                    let ConvertControl::Frame(frame, time) = control_rx
                        .blocking_recv()
                        .ok_or(eyre::eyre!("convert control closed"))?;

                    let dxgi_buffer =
                        MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &frame, 0, FALSE)?;

                    let sample = unsafe { MFCreateSample() }?;

                    sample.AddBuffer(&dxgi_buffer)?;
                    sample
                        .SetSampleTime(time.duration_since(UNIX_EPOCH)?.as_nanos() as i64 / 100)?;
                    // sample.SetSampleDuration(100_000_000 / target_framerate as i64)?;

                    let process_output = || {
                        let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();
                        output_buffer.dwStatus = 0;
                        output_buffer.dwStreamID = 0;
                        // output_buffer.pSample = ManuallyDrop::new(Some(output_sample.clone()));

                        let stream_output = transform.GetOutputStreamInfo(0)?;

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
                                log::warn!("output flags {}", output_buffers[0].dwStatus);
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

pub(crate) async fn convert_bgra_to_nv12(
    width: u32,
    height: u32,
) -> Result<(mpsc::Sender<ConvertControl>, mpsc::Receiver<ConvertEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "color-converter-control").await;
    telemetry::client::watch_channel(&event_tx, "color-converter-event").await;

    tokio::spawn(async move {
        match tokio::task::spawn_blocking(move || {
            let (device, context) = super::dx::create_device()?;

            let input_texture = super::dx::TextureBuilder::new(
                &device,
                width,
                height,
                super::dx::TextureFormat::BGRA,
            )
            .bind_shader_resource()
            .build()?;

            let output_texture = super::dx::TextureBuilder::new(
                &device,
                width,
                height,
                crate::media::dx::TextureFormat::NV12,
            )
            .bind_shader_resource()
            .build()?;

            let viewport = D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: width as f32,
                Height: height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };

            unsafe { context.RSSetViewports(Some(&[viewport])) };

            let sample_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MinLOD: 0.0,
                MaxLOD: f32::MAX,
                ..Default::default()
            };

            let mut sampler_state: Option<ID3D11SamplerState> = None;
            unsafe { device.CreateSamplerState(&sample_desc, Some(&mut sampler_state as *mut _)) }?;

            unsafe { context.PSSetSamplers(0, Some(&[sampler_state.clone()])) };

            let vertex_shader_blob =
                compile_shader(include_str!("cc.hlsl"), s!("vs_main"), s!("vs_5_0"))?;
            let mut vertex_shader: Option<ID3D11VertexShader> = None;
            unsafe {
                let vertex_shader_blob_buffer = std::slice::from_raw_parts(
                    vertex_shader_blob.GetBufferPointer() as *const u8,
                    vertex_shader_blob.GetBufferSize(),
                );
                device.CreateVertexShader(
                    vertex_shader_blob_buffer,
                    None,
                    Some(&mut vertex_shader as *mut _),
                )
            }?;

            let vertex_shader = vertex_shader.unwrap();

            let pixel_shader_blob =
                compile_shader(include_str!("cc.hlsl"), s!("main"), s!("ps_5_0"))?;
            let mut pixel_shader: Option<ID3D11PixelShader> = None;
            unsafe {
                let pixel_shader_blob_buffer = std::slice::from_raw_parts(
                    pixel_shader_blob.GetBufferPointer() as *const u8,
                    pixel_shader_blob.GetBufferSize(),
                );
                device.CreatePixelShader(
                    pixel_shader_blob_buffer,
                    None,
                    Some(&mut pixel_shader as *mut _),
                )
            }?;

            let pixel_shader = pixel_shader.unwrap();

            let pipeline_items = &[
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("POS"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 0,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("TEX"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: D3D11_APPEND_ALIGNED_ELEMENT,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
            ];

            let mut input_layout: Option<ID3D11InputLayout> = None;
            unsafe {
                let vertex_shader_blob_buffer = std::slice::from_raw_parts(
                    vertex_shader_blob.GetBufferPointer() as *const u8,
                    vertex_shader_blob.GetBufferSize(),
                );
                device.CreateInputLayout(
                    pipeline_items,
                    vertex_shader_blob_buffer,
                    Some(&mut input_layout as *mut _),
                )
            }?;

            let input_layout = input_layout.unwrap();

            #[repr(C)]
            struct Vertex {
                x: f32,
                y: f32,
                u: f32,
                v: f32,
            }

            let verticies = &[
                Vertex {
                    x: -1.0,
                    y: 1.0,
                    u: 0.0,
                    v: 0.0,
                },
                Vertex {
                    x: 1.0,
                    y: -1.0,
                    u: 1.0,
                    v: 1.0,
                },
                Vertex {
                    x: -1.0,
                    y: -1.0,
                    u: 0.0,
                    v: 1.0,
                },
                Vertex {
                    x: -1.0,
                    y: 1.0,
                    u: 0.0,
                    v: 0.0,
                },
                Vertex {
                    x: 1.0,
                    y: 1.0,
                    u: 1.0,
                    v: 0.0,
                },
                Vertex {
                    x: 1.0,
                    y: -1.0,
                    u: 1.0,
                    v: 1.0,
                },
            ];

            let topology = D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;

            let buffer_desc = D3D11_BUFFER_DESC {
                Usage: D3D11_USAGE_DEFAULT,
                ByteWidth: (verticies.len() * std::mem::size_of::<Vertex>()) as u32,
                BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
                ..Default::default()
            };

            let subresource_data = D3D11_SUBRESOURCE_DATA {
                pSysMem: verticies.as_ptr() as *const std::ffi::c_void,
                ..Default::default()
            };

            let mut vertex_buffer: Option<ID3D11Buffer> = None;

            unsafe {
                device.CreateBuffer(
                    &buffer_desc as *const _,
                    Some(&subresource_data as *const _),
                    Some(&mut vertex_buffer as *mut _),
                )
            }?;

            let mut input_texture_view: Option<ID3D11ShaderResourceView> = None;

            unsafe {
                let mut texture_view_desc: D3D11_SHADER_RESOURCE_VIEW_DESC =
                    D3D11_SHADER_RESOURCE_VIEW_DESC {
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                        ..Default::default()
                    };

                texture_view_desc.Anonymous.Texture2D = D3D11_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                };

                device.CreateShaderResourceView(
                    &input_texture,
                    Some(&texture_view_desc as *const _),
                    Some(&mut input_texture_view as *mut _),
                )
            }?;

            let mut texture_lum_view: Option<ID3D11ShaderResourceView> = None;

            unsafe {
                let mut texture_view_desc: D3D11_SHADER_RESOURCE_VIEW_DESC =
                    D3D11_SHADER_RESOURCE_VIEW_DESC {
                        Format: DXGI_FORMAT_R8_UNORM,
                        ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                        ..Default::default()
                    };

                texture_view_desc.Anonymous.Texture2D = D3D11_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                };

                device.CreateShaderResourceView(
                    &output_texture,
                    Some(&texture_view_desc as *const _),
                    Some(&mut texture_lum_view as *mut _),
                )
            }?;

            let mut texture_chrom_view: Option<ID3D11ShaderResourceView> = None;

            unsafe {
                let mut texture_view_desc: D3D11_SHADER_RESOURCE_VIEW_DESC =
                    D3D11_SHADER_RESOURCE_VIEW_DESC {
                        Format: DXGI_FORMAT_R8G8_UNORM,
                        ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                        ..Default::default()
                    };

                texture_view_desc.Anonymous.Texture2D = D3D11_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                };

                device.CreateShaderResourceView(
                    &output_texture,
                    Some(&texture_view_desc as *const _),
                    Some(&mut texture_chrom_view as *mut _),
                )
            }?;

            // let vertex_buffer = vertex_buffer.unwrap();
            let input_texture_view = input_texture_view.unwrap();
            let texture_lum_view = texture_lum_view.unwrap();
            let texture_chrom_view = texture_chrom_view.unwrap();

            loop {
                // Pull all frames out and use the latest
                let mut last_video = None;
                loop {
                    match control_rx.try_recv() {
                        Ok(frame) => last_video = Some(frame),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            log::error!("player disconnected");
                            Err(TryRecvError::Disconnected)?
                        }
                    }
                }

                if let Some(ConvertControl::Frame(video, time)) = last_video {
                    copy_texture(&input_texture, &video, None)?;

                    // unsafe { context.ClearRenderTargetView(&render_target, &[0.0, 0.0, 0.0, 1.0]) };

                    unsafe {
                        context.IASetInputLayout(&input_layout);
                        context.VSSetShader(&vertex_shader, None);
                        context.PSSetShader(&pixel_shader, None);
                    }

                    unsafe {
                        context.PSSetShaderResources(
                            0,
                            Some(&[
                                Some(input_texture_view.clone()),
                                Some(texture_lum_view.clone()),
                                Some(texture_chrom_view.clone()),
                            ]),
                        );
                        context.PSSetSamplers(0, Some(&[sampler_state.clone()]));
                    }

                    unsafe {
                        let mut offset = 0_u32;
                        let stride = std::mem::size_of::<Vertex>() as u32;
                        context.IASetVertexBuffers(
                            0,
                            1,
                            Some(&vertex_buffer as *const _),
                            Some(&stride as *const _),
                            Some(&mut offset),
                        );
                        context.IASetPrimitiveTopology(topology);
                    }

                    unsafe { context.Draw(verticies.len() as u32, 0) };

                    unsafe {
                        context.Flush();
                    };

                    // Copy texture out
                    let new_texture = super::dx::TextureBuilder::new(
                        &device,
                        width,
                        height,
                        crate::media::dx::TextureFormat::NV12,
                    )
                    .build()?;

                    copy_texture(&new_texture, &output_texture, None)?;

                    event_tx.blocking_send(ConvertEvent::Frame(new_texture, time))?;
                } else {
                    // log::warn!("no frame for player");
                    // log::error!("!!! no video frame waiting?")
                }
            }

            eyre::Ok(())
        })
        .await
        .unwrap()
        {
            Ok(ok) => {}
            Err(err) => {
                log::error!("Failed color convert {err} {err:?}")
            }
        }
    });

    Ok((control_tx, event_rx))
}
