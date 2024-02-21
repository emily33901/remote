use std::mem::MaybeUninit;

use eyre::Result;
use tokio::sync::mpsc;
use windows::{core::HSTRING, Win32::Media::MediaFoundation::*};

use crate::{VideoBuffer, ARBITRARY_CHANNEL_LIMIT};

use super::mf;

pub enum FileSinkControl {
    Video(VideoBuffer),
    Done,
}

pub fn file_sink(
    path: &std::path::Path,
    width: u32,
    height: u32,
    target_framerate: u32,
    target_bitrate: u32,
) -> Result<mpsc::Sender<FileSinkControl>> {
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let path = path.to_owned();

    tokio::spawn(async move {
        match tokio::task::spawn_blocking(move || unsafe {
            mf::init()?;

            let (device, _context) = super::dx::create_device()?;

            let mut reset_token = 0_u32;
            let mut device_manager: Option<IMFDXGIDeviceManager> = None;

            unsafe {
                MFCreateDXGIDeviceManager(&mut reset_token as *mut _, &mut device_manager as *mut _)
            }?;

            let device_manager = device_manager.unwrap();

            unsafe { device_manager.ResetDevice(&device, reset_token) }?;

            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 0)?;

            let attributes = attributes.unwrap();

            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            attributes.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &device_manager)?;

            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, None)?;

            {
                let output_type = MFCreateMediaType()?;
                output_type.SetGUID(
                    &MF_MT_MAJOR_TYPE as *const _,
                    &MFMediaType_Video as *const _,
                )?;
                output_type.SetGUID(&MF_MT_SUBTYPE as *const _, &MFVideoFormat_H264 as *const _)?;

                output_type.SetUINT32(&MF_MT_AVG_BITRATE as *const _, target_bitrate)?;

                let width_height = (width as u64) << 32 | (height as u64);
                output_type.SetUINT64(&MF_MT_FRAME_SIZE, width_height)?;

                let frame_rate = (target_framerate as u64) << 32 | 1_u64;
                output_type.SetUINT64(&MF_MT_FRAME_RATE as *const _, frame_rate)?;

                let pixel_aspect_ratio = (1_u64) << 32 | 1_u64;
                output_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO as *const _, pixel_aspect_ratio)?;

                // debug_video_format(&output_type)?;

                writer.AddStream(&output_type)?;
            }

            writer.BeginWriting()?;

            while let Some(control) = control_rx.blocking_recv() {
                match control {
                    FileSinkControl::Video(VideoBuffer {
                        data,
                        sequence_header: _,
                        time,
                        duration,
                        key_frame: _,
                    }) => {
                        let len = data.len();
                        let media_buffer = MFCreateMemoryBuffer(len as u32)?;

                        let mut begin: MaybeUninit<*mut u8> = MaybeUninit::uninit();
                        media_buffer.Lock(&mut begin as *mut _ as *mut *mut u8, None, None)?;

                        let begin = begin.assume_init();

                        std::ptr::copy(data.as_ptr(), begin, len as usize);

                        media_buffer.SetCurrentLength(len as u32)?;
                        media_buffer.Unlock()?;

                        let sample = MFCreateSample()?;

                        sample.AddBuffer(&media_buffer)?;
                        sample.SetSampleTime(time.hns())?;

                        sample.SetSampleDuration(duration.as_nanos() as i64 / 100)?;
                        writer.WriteSample(0, &sample)?;
                        // writer.Flush(0)?;
                    }
                    FileSinkControl::Done => {
                        writer.Finalize()?;
                        break;
                    }
                }
            }

            eyre::Ok(())
        })
        .await
        .unwrap()
        {
            Ok(_ok) => log::debug!("file_sink died ok"),
            Err(err) => log::error!("file_sink died with error {err}"),
        };
    });

    Ok(control_tx)
}
