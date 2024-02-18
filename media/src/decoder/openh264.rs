use std::cell::RefCell;

use openh264::{
    decoder::{Decoder, DecoderConfig},
    nal_units, OpenH264API,
};
use tokio::sync::mpsc;

use crate::{dx::MapTextureExt, yuv_buffer::YUVBuffer2, ARBITRARY_CHANNEL_LIMIT};

use super::{DecoderControl, DecoderEvent};

use eyre::{eyre, Result};

// fn i420_to_nv12(
//     width: usize,
//     height: usize,
//     i420_data: &[u8],
//     src_y_stride: usize,
//     dest_y_stride: usize,
//     dest_uv_stride: usize,
//     nv12_data: &mut [u8],
// ) {
//     let y_size = dest_y_stride * height;
//     let uv_size = dest_uv_stride * height / 2;

//     // Extract Y, U, and V planes
//     let (y_plane, uv_plane) = i420_data.split_at(y_size);
//     let (u_plane, v_plane) = uv_plane.split_at(uv_size);

//     // Copy Y plane with source and destination strides
//     for row in 0..height {
//         let src_offset = row * src_y_stride;
//         let dest_offset = row * dest_y_stride;
//         nv12_data[dest_offset..dest_offset + width].copy_from_slice(&y_plane[src_offset..src_offset + width]);
//     }

//     // Interleave U and V planes into UV plane with destination stride
//     for row in 0..height / 2 {
//         let dest_offset = row * dest_uv_stride;

//         for col in 0..width / 2 {
//             let uv_index = col * 2;
//             let dest_index = dest_offset + col;

//             nv12_data[y_size + dest_index] = u_plane[uv_index];
//             nv12_data[y_size + dest_index + 1] = v_plane[uv_index];
//         }
//     }
// }

fn i420_components_to_nv12(
    width: usize,
    height: usize,
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    src_y_stride: usize,
    src_u_stride: usize,
    src_v_stride: usize,
    dest_y_stride: usize,
    dest_uv_stride: usize,
    nv12_data: &mut [u8],
) {
    let y_size = dest_y_stride * height;
    let uv_size = dest_uv_stride * height / 2;

    // Copy Y plane with source and destination strides
    for row in 0..height {
        let src_offset = row * src_y_stride;
        let dest_offset = row * dest_y_stride;
        nv12_data[dest_offset..dest_offset + width]
            .copy_from_slice(&y_plane[src_offset..src_offset + width]);
    }

    // Copy U plane with source and destination strides
    for row in 0..height / 2 {
        let src_offset = row * src_u_stride;
        let dest_offset = row * dest_uv_stride;
        nv12_data[y_size + dest_offset..y_size + dest_offset + width / 2]
            .copy_from_slice(&u_plane[src_offset..src_offset + width / 2]);
    }

    // Copy V plane with source and destination strides
    for row in 0..height / 2 {
        let src_offset = row * src_v_stride;
        let dest_offset = row * dest_uv_stride;
        nv12_data[y_size + dest_offset + 1..y_size + dest_offset + width / 2 + 1]
            .copy_from_slice(&v_plane[src_offset..src_offset + width / 2]);
    }
}

pub async fn h264_decoder(
    width: u32,
    height: u32,
    target_framerate: u32,
    target_bitrate: u32,
) -> Result<(mpsc::Sender<DecoderControl>, mpsc::Receiver<DecoderEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "h264-decoder-control").await;
    telemetry::client::watch_channel(&event_tx, "h264-decoder-event").await;

    tokio::spawn(async move {
        let fps_counter = telemetry::client::Counter::default();
        telemetry::client::watch_counter(&fps_counter, telemetry::Unit::Fps, "decoder fps").await;

        match tokio::task::spawn_blocking(move || {
            let (device, context) = crate::dx::create_device()?;

            let config = DecoderConfig::new();

            let api = OpenH264API::from_source();
            let mut decoder = Decoder::with_config(api, config)?;

            let staging_texture = crate::dx::TextureBuilder::new(
                &device,
                width,
                height,
                crate::dx::TextureFormat::NV12,
            )
            .cpu_access(crate::dx::TextureCPUAccess::Write)
            .usage(crate::dx::TextureUsage::Staging)
            .build()?;

            loop {
                let DecoderControl::Data(buffer) = control_rx
                    .blocking_recv()
                    .ok_or(eyre!("decoder control closed"))?;

                for unit in nal_units(&buffer.data) {
                    if let Ok(Some(output)) = decoder.decode(unit) {
                        staging_texture.map_mut(&context, |data, dest_stride| {
                            let (y_stride, u_stride, v_stride) = output.strides_yuv();

                            i420_components_to_nv12(
                                width as usize,
                                height as usize,
                                output.y_with_stride(),
                                output.u_with_stride(),
                                output.v_with_stride(),
                                y_stride,
                                u_stride,
                                v_stride,
                                dest_stride,
                                dest_stride,
                                data,
                            );

                            let frame = crate::dx::TextureBuilder::new(
                                &device,
                                width,
                                height,
                                crate::dx::TextureFormat::NV12,
                            )
                            .keyed_mutex()
                            .nt_handle()
                            .build()?;

                            crate::dx::copy_texture(&frame, &staging_texture, None)?;

                            event_tx.blocking_send(DecoderEvent::Frame(
                                frame,
                                std::time::SystemTime::now(),
                            ))?;

                            fps_counter.update(1);

                            Ok(())
                        })?;
                    }
                }
            }

            eyre::Ok(())
        })
        .await
        .unwrap()
        {
            Ok(_) => log::info!("h264 decoder down ok"),
            Err(err) => log::warn!("h264 encoder down err {err} {err:?}"),
        }
    });

    Ok((control_tx, event_rx))
}