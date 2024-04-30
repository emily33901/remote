use openh264::{
    decoder::{Decoder, DecoderConfig},
    formats::YUVSource,
    nal_units, OpenH264API,
};
use tokio::sync::mpsc;

use crate::{dx::ID3D11Texture2DExt, ARBITRARY_MEDIA_CHANNEL_LIMIT};

use super::{DecoderControl, DecoderEvent};

use eyre::{eyre, Result};

fn i420_components_to_nv12(
    width: usize,
    height: usize,
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    src_y_stride: usize,
    src_u_stride: usize,
    _src_v_stride: usize,
    dest_y_stride: usize,
    dest_uv_stride: usize,
    nv12_data: &mut [u8],
) {
    let y_size = dest_y_stride * height;
    let _uv_size = dest_uv_stride * height / 2;

    for row in 0..height {
        let src_offset = row * src_y_stride;
        let dest_offset = row * dest_y_stride;
        nv12_data[dest_offset..dest_offset + width]
            .copy_from_slice(&y_plane[src_offset..src_offset + width]);
    }

    for row in 0..height / 2 {
        let src_offset = row * src_u_stride;
        let dest_offset = row * dest_uv_stride;
        for i in 0..width / 2 {
            nv12_data[y_size + dest_offset + i * 2] = u_plane[src_offset + i];
            nv12_data[y_size + dest_offset + i * 2 + 1] = v_plane[src_offset + i];
        }
    }

    // // Interleave U and V planes into UV plane
    // for i in 0..uv_size / 2 {
    //     nv12_data[y_size + i * 2] = u_plane[i];
    //     nv12_data[y_size + i * 2 + 1] = v_plane[i];
    // }

    // for row in 0..(height / 2) {
    //     let src_offset_u = row * src_u_stride;
    //     let src_offset_v = row * src_v_stride;
    //     let dest_offset = row * dest_uv_stride;
    //     for col in (0..width - 1).step_by(2) {
    //         nv12_data[y_size + dest_offset + col] = u_plane[src_offset_u + col];
    //         nv12_data[y_size + dest_offset + col + 1] = v_plane[src_offset_v + col];
    //     }
    // }
}

pub async fn h264_decoder(
    width: u32,
    height: u32,
    _target_framerate: u32,
    _target_bitrate: u32,
) -> Result<(mpsc::Sender<DecoderControl>, mpsc::Receiver<DecoderEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);

    telemetry::client::watch_channel(&control_tx, "h264-decoder-control").await;
    telemetry::client::watch_channel(&event_tx, "h264-decoder-event").await;

    tokio::spawn(async move {
        let fps_counter = telemetry::client::Counter::default();
        telemetry::client::watch_counter(&fps_counter, telemetry::Unit::Fps, "decoder fps").await;

        match tokio::task::spawn_blocking(move || {
            let (device, context) = crate::dx::create_device()?;

            let config = DecoderConfig::new();

            let api = OpenH264API::from_source();
            let mut decoder = Decoder::with_api_config(api, config)?;

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
                        tracing::debug!("decoder timestamp was {:?}", output.timestamp());

                        staging_texture.map_mut(&context, |data, dest_stride| {
                            let (y_stride, u_stride, v_stride) = output.strides();

                            i420_components_to_nv12(
                                width as usize,
                                height as usize,
                                output.y(),
                                output.u(),
                                output.v(),
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
                                crate::Timestamp::new_millis(output.timestamp().as_millis()),
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
            Ok(_) => tracing::info!("h264 decoder down ok"),
            Err(err) => tracing::warn!("h264 encoder down err {err} {err:?}"),
        }
    });

    Ok((control_tx, event_rx))
}
