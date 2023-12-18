use std::{mem::MaybeUninit, time::UNIX_EPOCH};

use eyre::Result;
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

use crate::{media::produce::debug_video_format, video::VideoBuffer, ARBITRARY_CHANNEL_LIMIT};

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
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

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
                    MFT_CATEGORY_VIDEO_ENCODER,
                    MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
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
                let activate = activates.first().unwrap().as_ref().unwrap();

                let transform: IMFTransform = activate.ActivateObject()?;

                let attributes = transform.GetAttributes()?;

                attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;

                let event_gen: IMFMediaEventGenerator = transform.cast()?;

                let mut input_stream_ids = [0];
                let mut output_stream_ids = [0];

                transform.GetStreamIDs(&mut input_stream_ids, &mut output_stream_ids)?;

                transform.ProcessMessage(
                    MFT_MESSAGE_SET_D3D_MANAGER,
                    std::mem::transmute(device_manager),
                )?;

                attributes.SetUINT32(&MF_LOW_LATENCY as *const _, 1)?;

                {
                    let output_type = MFCreateMediaType()?;
                    output_type.SetGUID(
                        &MF_MT_MAJOR_TYPE as *const _,
                        &MFMediaType_Video as *const _,
                    )?;
                    output_type
                        .SetGUID(&MF_MT_SUBTYPE as *const _, &MFVideoFormat_H264 as *const _)?;

                    output_type.SetUINT32(&MF_MT_AVG_BITRATE as *const _, target_bitrate)?;

                    let width_height = (width as u64) << 32 | (height as u64);
                    output_type.SetUINT64(&MF_MT_FRAME_SIZE, width_height)?;

                    let frame_rate = (target_framerate as u64) << 32 | 1_u64;
                    output_type.SetUINT64(&MF_MT_FRAME_RATE as *const _, frame_rate)?;

                    output_type
                        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                    // output_type.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, 100)?;
                    output_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;

                    output_type
                        .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;

                    let pixel_aspect_ratio = (1_u64) << 32 | 1_u64;
                    output_type
                        .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO as *const _, pixel_aspect_ratio)?;

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
                    input_type.SetUINT64(&MF_MT_FRAME_RATE as *const _, frame_rate)?;

                    debug_video_format(&input_type)?;

                    transform.SetInputType(input_stream_ids[0], &input_type, 0)?;
                }

                transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
                transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
                transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

                // let output_sample = unsafe { MFCreateSample() }?;

                // let output_media_buffer = unsafe { MFCreateMemoryBuffer(width * height * 4) }?;
                // unsafe { output_sample.AddBuffer(&output_media_buffer.clone()) }?;

                let _status = 0;

                loop {
                    let event = event_gen.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))?;
                    let event_type = event.GetType()?;

                    match event_type {
                        601 => {
                            let EncoderControl::Frame(frame, time) = control_rx
                                .blocking_recv()
                                .ok_or(eyre::eyre!("encoder control closed"))?;

                            {
                                let dxgi_buffer = MFCreateDXGISurfaceBuffer(
                                    &ID3D11Texture2D::IID,
                                    &frame,
                                    0,
                                    FALSE,
                                )?;

                                let sample = unsafe { MFCreateSample() }?;

                                sample.AddBuffer(&dxgi_buffer)?;
                                sample.SetSampleTime(
                                    time.duration_since(UNIX_EPOCH)?.as_nanos() as i64 / 100,
                                )?;
                                sample.SetSampleDuration(100_000_000 / target_framerate as i64)?;

                                transform.ProcessInput(input_stream_ids[0], &sample, 0)?;
                            }
                        }

                        // METransformHaveOutput
                        602 => {
                            let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();
                            // output_buffer.pSample =
                            // ManuallyDrop::new(Some(output_sample.clone())); //  ManuallyDrop::new(Some(sample.clone()));
                            output_buffer.dwStatus = 0;
                            output_buffer.dwStreamID = output_stream_ids[0];

                            let mut output_buffers = [output_buffer];

                            let mut status = 0_u32;
                            match transform.ProcessOutput(0, &mut output_buffers, &mut status) {
                                Ok(_ok) => {
                                    // let timestamp = unsafe { sample.GetSampleTime()? };
                                    let sample = output_buffers[0].pSample.take().unwrap();
                                    let media_buffer =
                                        unsafe { sample.ConvertToContiguousBuffer() }?;

                                    // let sample = &output_sample;
                                    // let media_buffer = &output_media_buffer;

                                    let mut output = vec![];
                                    let mut sequence_header = None;

                                    let sample_time;
                                    let duration;

                                    unsafe {
                                        let is_keyframe =
                                            sample.GetUINT32(&MFSampleExtension_CleanPoint)? == 1;
                                        // let is_keyframe = false;

                                        sample_time =
                                            sample.GetUINT64(&MFSampleExtension_DecodeTimestamp)?;

                                        duration = std::time::Duration::from_nanos(
                                            sample.GetSampleDuration()? as u64 * 100,
                                        );

                                        let mut begin: MaybeUninit<*mut u8> = MaybeUninit::uninit();
                                        let mut len = media_buffer.GetCurrentLength()?;
                                        let mut max_len = media_buffer.GetMaxLength()?;

                                        media_buffer.Lock(
                                            &mut begin as *mut _ as *mut *mut u8,
                                            Some(&mut len),
                                            Some(&mut max_len),
                                        )?;
                                        // log::info!(
                                        //     "h264 buffer len is {len} (max len is {max_len})"
                                        // );

                                        output.resize(len as usize, 0);
                                        let begin = begin.assume_init();

                                        sequence_header = if is_keyframe {
                                            // log::info!("keyframe!");
                                            let output_type = transform
                                                .GetOutputCurrentType(output_stream_ids[0])?;
                                            let extra_data_size = output_type
                                                .GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER)?
                                                as usize;

                                            let mut sequence_header = vec![0; extra_data_size];

                                            output_type.GetBlob(
                                                &MF_MT_MPEG_SEQUENCE_HEADER,
                                                &mut sequence_header.as_mut_slice()
                                                    [..extra_data_size],
                                                None,
                                            )?;

                                            Some(sequence_header)
                                        } else {
                                            None
                                        };

                                        std::ptr::copy(begin, output.as_mut_ptr(), len as usize);

                                        media_buffer.Unlock()?;
                                    };

                                    event_tx.blocking_send(EncoderEvent::Data(VideoBuffer {
                                        data: output,
                                        sequence_header: sequence_header,
                                        time: std::time::UNIX_EPOCH
                                            + std::time::Duration::from_nanos(sample_time * 100),
                                        duration: duration,
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
