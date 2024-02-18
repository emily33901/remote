use std::cell::RefCell;

use openh264::{
    self,
    encoder::{Encoder, EncoderConfig},
    formats::{YUVBuffer, YUVSource},
    OpenH264API,
};

use eyre::{eyre, Result};
use tokio::sync::mpsc;

use crate::{dx::MapTextureExt, ARBITRARY_CHANNEL_LIMIT};

use crate::yuv_buffer::{self, YUVBuffer2};

use super::{EncoderControl, EncoderEvent};

// Copy a plane of data
#[inline(never)]
fn copy_plane(
    mut src_y: &[u8],
    mut src_stride_y: usize,
    mut dst_y: &mut [u8],
    mut dst_stride_y: usize,
    mut width: usize,
    mut height: usize,
) {
    if (width <= 0 || height == 0) {
        return;
    }
    // Negative height means invert the image.
    //   if (height < 0) {
    //     height = -height;
    //     dst_y = dst_y + (height - 1) * dst_stride_y;
    //     dst_stride_y = -dst_stride_y;
    //   }
    // // Coalesce rows.
    // if (src_stride_y == width && dst_stride_y == width) {
    //     width *= height;
    //     height = 1;
    //     src_stride_y = 0;
    //     dst_stride_y = 0;
    // }
    // // Nothing to do.
    // if (src_y == dst_y && src_stride_y == dst_stride_y) {
    //     return;
    // }

    for y in 0..height {
        dst_y[..width].copy_from_slice(&src_y[..width]);
        src_y = &src_y[src_stride_y..];
        dst_y = &mut dst_y[dst_stride_y..];
    }
}

#[inline(never)]
fn split_uv_row_c(mut src_uv: &[u8], dst_u: &mut [u8], dst_v: &mut [u8], width: usize) {
    for x in (0..width - 1).step_by(2) {
        dst_u[x] = src_uv[0];
        dst_u[x + 1] = src_uv[2];
        dst_v[x] = src_uv[1];
        dst_v[x + 1] = src_uv[3];
        src_uv = &src_uv[4..];
    }

    if width & 1 != 0 {
        dst_u[width - 1] = src_uv[0];
        dst_v[width - 1] = src_uv[1];
    }
}

#[inline(never)]
fn split_uv_plane(
    mut src_uv: &[u8],
    mut src_stride_uv: usize,
    mut dst_u: &mut [u8],
    mut dst_stride_u: usize,
    mut dst_v: &mut [u8],
    mut dst_stride_v: usize,
    mut width: usize,
    mut height: usize,
) {
    // int y;
    // void (*SplitUVRow)(const uint8_t* src_uv, uint8_t* dst_u, uint8_t* dst_v,
    //                    int width) = SplitUVRow_C;
    if (width <= 0 || height == 0) {
        return;
    }
    // Negative height means invert the image.
    // if (height < 0) {
    //     height = -height;
    //     dst_u = dst_u + (height - 1) * dst_stride_u;
    //     dst_v = dst_v + (height - 1) * dst_stride_v;
    //     dst_stride_u = -dst_stride_u;
    //     dst_stride_v = -dst_stride_v;
    // }
    // Coalesce rows.
    // if (src_stride_uv == width * 2 && dst_stride_u == width && dst_stride_v == width) {
    //     width *= height;
    //     height = 1;
    //     src_stride_uv = 0;
    //     dst_stride_u = 0;
    //     dst_stride_v = 0;
    // }

    for y in 0..height {
        // Copy a row of UV.
        split_uv_row_c(src_uv, dst_u, dst_v, width);
        dst_u = &mut dst_u[dst_stride_u..];
        dst_v = &mut dst_v[dst_stride_v..];
        src_uv = &src_uv[src_stride_uv..];
    }
}

fn nv12_to_i420_inner(
    nv12: &[u8],
    dst_y: &mut [u8],
    mut dst_stride_y: usize,
    dst_u: &mut [u8],
    mut dst_stride_u: usize,
    dst_v: &mut [u8],
    mut dst_stride_v: usize,
    mut width: usize,
    mut height: usize,
) {
    let mut halfwidth = (width + 1) >> 2;
    let mut halfheight = (height + 1) >> 2;
    let src_y = &nv12[..];
    let mut src_stride_y = 2048;
    let src_uv = &nv12[(width * height)..];
    let mut src_stride_uv = 2048;

    // if (src_stride_y == width && dst_stride_y == width) {
    //     width *= height;
    //     height = 1;
    //     src_stride_y = 0;
    //     dst_stride_y = 0;
    // }
    // // // Coalesce rows.
    // if (src_stride_uv == halfwidth * 2 && dst_stride_u == halfwidth && dst_stride_v == halfwidth) {
    //     halfwidth *= halfheight;
    //     halfheight = 1;
    //     src_stride_uv = 0;
    //     dst_stride_u = 0;
    //     dst_stride_v = 0;
    // }
    // // // Coalesce rows.
    // if (src_stride_uv == halfwidth * 2 && dst_stride_u == halfwidth && dst_stride_v == halfwidth) {
    //     halfwidth *= halfheight;
    //     halfheight = 1;
    //     src_stride_uv = 0;
    //     dst_stride_u = 0;
    //     dst_stride_v = 0;
    // }
    // copy_plane(src_y, src_stride_y, dst_y, dst_stride_y, width, height);
    // Split UV plane - NV12 / NV21
    split_uv_plane(
        src_uv,
        src_stride_uv,
        dst_u,
        dst_stride_u,
        dst_v,
        dst_stride_v,
        halfwidth,
        halfheight,
    );
}

// fn nv12_to_i420(nv12: &[u8], i420: &mut [u8], mut width: usize, mut height: usize) {
//     let mut halfwidth = (width + 1) >> 2;
//     let mut halfheight = (height + 1) >> 2;
//     let src_y = &nv12[..];
//     let mut src_stride_y = width;
//     let src_uv = &nv12[(width * height)..];
//     let mut src_stride_uv = width;

//     let dst_y = &mut i420[..];
//     let mut dst_stride_y = width;
//     let dst_u = &mut i420[(width * height)..];
//     let mut dst_stride_u = width >> 1;
//     let dst_v = &mut i420[((width * height) + (width * height / 4))..];
//     let mut dst_stride_v = height >> 1;

//     if (src_stride_y == width && dst_stride_y == width) {
//         width *= height;
//         height = 1;
//         src_stride_y = 0;
//         dst_stride_y = 0;
//     }
//     // Coalesce rows.
//     if (src_stride_uv == halfwidth * 2 && dst_stride_u == halfwidth && dst_stride_v == halfwidth) {
//         halfwidth *= halfheight;
//         halfheight = 1;
//         src_stride_uv = 0;
//         dst_stride_u = 0;
//         dst_stride_v = 0;
//     }
//     // Coalesce rows.
//     if (src_stride_uv == halfwidth * 2 && dst_stride_u == halfwidth && dst_stride_v == halfwidth) {
//         halfwidth *= halfheight;
//         halfheight = 1;
//         src_stride_uv = 0;
//         dst_stride_u = 0;
//         dst_stride_v = 0;
//     }
//     copy_plane(src_y, src_stride_y, dst_y, dst_stride_y, width, height);
//     // Split UV plane - NV12 / NV21
//     split_uv_plane(
//         src_uv,
//         src_stride_uv,
//         dst_u,
//         dst_stride_u,
//         dst_v,
//         dst_stride_v,
//         halfwidth,
//         halfheight,
//     );
// }

// fn nv12_to_i420(
//     width: usize,
//     height: usize,
//     source_row_pitch: usize,
//     nv12_data: &[u8],
//     i420_data: &mut [u8],
// ) {
//     let y_size = source_row_pitch * height;
//     let uv_size = (source_row_pitch * (height / 2));

//     let y_plane = &nv12_data[..y_size];
//     let uv_plane = &nv12_data[y_size..y_size + (uv_size * 2)];

//     let (u_plane, v_plane) = i420_data[(width * height)..].split_at_mut((width * height) / 4);

//     for row in 0..height / 2 {
//         let src_uv_offset = row * source_row_pitch;
//         let dest_uv_offset = row * width / 2;

//         // split_uv_plane(
//         //     uv_plane,
//         //     source_row_pitch,
//         //     u_plane,
//         //     width,
//         //     v_plane,
//         //     width,
//         //     (width + 1 >> 2),
//         //     (height + 1 >> 2),
//         // );

//         // for ((u_src, v_src), (u_dst, v_dst)) in (uv_plane
//         //     .iter()
//         //     .step_by(2)
//         //     .zip(uv_plane.iter().step_by(2).skip(1))
//         //     .zip(u_plane.iter_mut().zip(v_plane.iter_mut())))
//         // {
//         //     *u_dst = *u_src;
//         //     *v_dst = *v_src;
//         // }

//         for col in 0..width / 2 {
//             let uv_index = col * 2;
//             let dest_index = dest_uv_offset + col;

//             u_plane[dest_index] = uv_plane[src_uv_offset + uv_index];
//             v_plane[dest_index] = uv_plane[src_uv_offset + uv_index + 1];
//         }

//         // u_plane[dest_uv_offset..dest_uv_offset + width / 2]
//         //     .copy_from_slice(&uv_plane[src_uv_offset..src_uv_offset + width / 2]);

//         // let src = &uv_plane
//         //     [src_uv_offset + source_row_pitch..src_uv_offset + source_row_pitch + width / 2];

//         // v_plane[dest_uv_offset..dest_uv_offset + width / 2].copy_from_slice(src);
//     }

//     for row in 0..height {
//         let src_y_offset = row * source_row_pitch;
//         let dest_y_offset = row * width;

//         i420_data[dest_y_offset..dest_y_offset + width]
//             .copy_from_slice(&y_plane[src_y_offset..src_y_offset + width]);
//     }
// }

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

pub async fn h264_encoder(
    width: u32,
    height: u32,
    target_framerate: u32,
    target_bitrate: u32,
) -> Result<(mpsc::Sender<EncoderControl>, mpsc::Receiver<EncoderEvent>)> {
    let (event_tx, event_rx) = mpsc::channel::<EncoderEvent>(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel::<EncoderControl>(ARBITRARY_CHANNEL_LIMIT);

    let fps_counter = telemetry::client::Counter::default();
    telemetry::client::watch_counter(&fps_counter, telemetry::Unit::Fps, "encoder fps").await;

    tokio::task::spawn_blocking(move || {
        let (device, context) = crate::dx::create_device()?;

        let config = EncoderConfig::new(width, height)
            .max_frame_rate(target_framerate as f32)
            .set_bitrate_bps(target_bitrate);

        let api = OpenH264API::from_source();
        let mut encoder = Encoder::with_config(api, config)?;

        let yuv_buffer = RefCell::new(YUVBuffer2::new(width as usize, height as usize));

        let staging_texture =
            crate::dx::TextureBuilder::new(&device, width, height, crate::dx::TextureFormat::NV12)
                .cpu_access(crate::dx::TextureCPUAccess::Read)
                .usage(crate::dx::TextureUsage::Staging)
                .build()?;

        loop {
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

            let EncoderControl::Frame(frame, time) = control;

            {
                crate::dx::copy_texture(&staging_texture, &frame, None)?;

                staging_texture.map(&context, |data, source_row_pitch| {
                    // TODO(emily): You actually need to obey row pitch and depth pitch here!
                    let mut yuv_buffer = yuv_buffer.borrow_mut();

                    // let y_stride = yuv_buffer.y_stride() as usize;
                    // let u_stride = yuv_buffer.u_stride() as usize;
                    // let v_stride = yuv_buffer.v_stride() as usize;

                    // let (y, u, v) = yuv_buffer.yuv_mut();

                    // nv12_to_i420_inner(
                    //     data,
                    //     y,
                    //     y_stride,
                    //     u,
                    //     u_stride,
                    //     v,
                    //     v_stride,
                    //     width as usize,
                    //     height as usize,
                    // );

                    nv12_to_i420(
                        width as usize,
                        height as usize,
                        data,
                        source_row_pitch,
                        yuv_buffer.y_stride() as usize,
                        yuv_buffer.u_stride() as usize,
                        yuv_buffer.buffer_mut(),
                    );

                    Ok(())
                })?;

                let yuv_buffer = yuv_buffer.borrow();

                let bitstream = encoder.encode(&*yuv_buffer)?;

                log::info!("{} layers", bitstream.num_layers());

                fps_counter.update(1);

                event_tx
                    .blocking_send(EncoderEvent::Data(crate::VideoBuffer {
                        data: bitstream.to_vec(),
                        sequence_header: None,
                        time: time,
                        duration: std::time::Duration::from_secs_f32(1.0 / target_framerate as f32),
                        key_frame: super::FrameIsKeyframe::No,
                    }))
                    .unwrap();
            }
        }

        eyre::Ok(())
    });

    Ok((control_tx, event_rx))
}
