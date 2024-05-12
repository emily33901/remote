use std::{
    cell::RefCell,
    time::{Instant, SystemTime},
};

use openh264::{
    self,
    encoder::{Encoder, EncoderConfig},
    formats::YUVSource,
    OpenH264API,
};

use eyre::{eyre, Result};
use tokio::sync::mpsc;
use tracing::Instrument;

use crate::{
    dx::ID3D11Texture2DExt,
    statistics::{self, EncodeStatistics},
    RateControlMode, ARBITRARY_MEDIA_CHANNEL_LIMIT,
};

use crate::yuv_buffer::YUVBuffer2;

use super::{EncoderControl, EncoderEvent};

fn nv12_to_i420(
    width: usize,
    height: usize,
    nv12_data: &[u8],
    src_row_pitch: usize,
    dest_y_stride: usize,
    dest_uv_stride: usize,
    i420_data: &mut [u8],
) {
    // Extract Y and interleaved UV components
    let (y_plane, uv_plane) = nv12_data.split_at(src_row_pitch * height);

    // Copy Y plane with stride
    for row in 0..height {
        let src_offset = row * src_row_pitch;
        let dest_offset = row * dest_y_stride;
        i420_data[dest_offset..dest_offset + width]
            .copy_from_slice(&y_plane[src_offset..src_offset + width]);
    }

    let y_size = dest_y_stride * height;
    let uv_size = dest_uv_stride * height / 2;

    // Separate interleaved UV into U and V planes
    let (u_plane, v_plane) = i420_data[y_size..].split_at_mut(uv_size);

    // Deinterleave UV plane with stride
    for row in 0..height / 2 {
        let src_offset = row * src_row_pitch;
        let dest_offset = row * dest_uv_stride;

        for col in 0..width / 2 {
            let uv_index = col * 2;
            let dest_index = dest_offset + col;

            u_plane[dest_index] = uv_plane[src_offset + uv_index];
            v_plane[dest_index] = uv_plane[src_offset + uv_index + 1];
        }
    }
}

#[tracing::instrument]
pub async fn h264_encoder(
    width: u32,
    height: u32,
    target_framerate: u32,
    rate_control: RateControlMode,
) -> Result<(mpsc::Sender<EncoderControl>, mpsc::Receiver<EncoderEvent>)> {
    let (event_tx, event_rx) = mpsc::channel::<EncoderEvent>(ARBITRARY_MEDIA_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) =
        mpsc::channel::<EncoderControl>(ARBITRARY_MEDIA_CHANNEL_LIMIT);

    tokio::task::spawn_blocking(move || {
        let (device, context) = crate::dx::create_device()?;

        let config = EncoderConfig::new()
            .max_frame_rate(target_framerate as f32)
            .enable_skip_frame(true);

        let config = match rate_control {
            RateControlMode::Bitrate(bitrate) => config
                .rate_control_mode(openh264::encoder::RateControlMode::Bitrate)
                .set_bitrate_bps(bitrate),
            RateControlMode::Quality(quality) => {
                config
                    .rate_control_mode(openh264::encoder::RateControlMode::Quality)
                    .set_bitrate_bps(8_000_000)
                // TODO(emily): OpenH264-rs has no way to set quality param
            }
        };

        let api = OpenH264API::from_source();
        let mut encoder = Encoder::with_api_config(api, config)?;

        let yuv_buffer = RefCell::new(YUVBuffer2::new(width as usize, height as usize));

        let staging_texture =
            crate::dx::TextureBuilder::new(&device, width, height, crate::dx::TextureFormat::NV12)
                .cpu_access(crate::dx::TextureCPUAccess::Read)
                .usage(crate::dx::TextureUsage::Staging)
                .build()?;

        loop {
            // TODO(emily): Like in the media foundation encoder, is this the right thing to do?
            let control = {
                let mut control = None;
                while let Ok(c) = control_rx.try_recv() {
                    control = Some(c);
                }

                if let None = control {
                    control = Some(
                        control_rx
                            .blocking_recv()
                            .ok_or(eyre!("control_rx gone down"))?,
                    );
                }

                control.unwrap()
            };

            let EncoderControl::Frame(frame, time, statistics) = control;

            {
                let input_time = Instant::now();

                crate::dx::copy_texture(&staging_texture, &frame, None)?;

                staging_texture.map(&context, |data, source_row_pitch| {
                    let mut yuv_buffer = yuv_buffer.borrow_mut();
                    let (y_stride, u_stride, v_stride) = yuv_buffer.strides();
                    nv12_to_i420(
                        width as usize,
                        height as usize,
                        data,
                        source_row_pitch,
                        y_stride,
                        u_stride,
                        yuv_buffer.buffer_mut(),
                    );

                    Ok(())
                })?;

                let yuv_buffer = yuv_buffer.borrow();

                let bitstream = encoder.encode_at(
                    &*yuv_buffer,
                    openh264::Timestamp::from_millis(time.duration().as_millis() as u64),
                )?;

                event_tx.blocking_send(EncoderEvent::Data(crate::VideoBuffer {
                    data: bitstream.to_vec(),
                    sequence_header: None,
                    time: time,
                    duration: std::time::Duration::from_secs_f32(1.0 / target_framerate as f32),
                    key_frame: super::FrameIsKeyframe::No,
                    statistics: crate::Statistics {
                        encode: Some(EncodeStatistics {
                            media_queue_len: 0,
                            time: input_time.elapsed(),
                            end_time: SystemTime::now(),
                        }),
                        ..statistics
                    },
                }))?;
            }
        }

        eyre::Ok(())
    });

    Ok((control_tx, event_rx))
}
