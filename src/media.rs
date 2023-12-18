use tokio::sync::mpsc;
use windows::Win32::{
    Graphics::Direct3D11::{D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ},
    System::Com::{CoInitializeEx, COINIT_DISABLE_OLE1DDE},
};

pub(crate) mod dx;

mod produce;

pub(crate) mod decoder;
pub(crate) mod encoder;

pub(crate) mod byte_stream;
pub(crate) mod file_sink;

use eyre::Result;

use crate::{video::VideoBuffer, ARBITRARY_CHANNEL_LIMIT};

pub(crate) enum MediaEvent {
    Audio(Vec<u8>),
    Video(VideoBuffer),
}

pub(crate) enum MediaControl {}

pub(crate) async fn produce(
    path: &str,
    width: u32,
    height: u32,
    bitrate: u32,
) -> Result<(mpsc::Sender<MediaControl>, mpsc::Receiver<MediaEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    tokio::spawn(async move {
        while let Some(control) = control_rx.recv().await {
            match control {}
        }
    });

    let (h264_control, mut h264_event) = encoder::h264_encoder(width, height, 30, bitrate).await?;

    tokio::spawn({
        let event_tx = event_tx.clone();
        let path = path.to_owned();
        async move {
            match tokio::task::spawn_blocking({
                move || {
                    unsafe {
                        CoInitializeEx(None, COINIT_DISABLE_OLE1DDE)?;
                    }

                    let (device, context) = dx::create_device()?;

                    let texture = dx::create_texture_sync(
                        &device,
                        width,
                        height,
                        windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12,
                    )?;

                    let path = path.to_owned();
                    let mut media = produce::Media::new(&device, &path, width, height)?;

                    media.debug_media_format()?;

                    let mut deadline: Option<std::time::SystemTime> = None;
                    let mut prev = std::time::Instant::now();
                    let start = std::time::SystemTime::now();

                    let mut audio_buffer: Vec<u8> = vec![];

                    loop {
                        if let Some(deadline) = deadline {
                            if let Ok(duration) =
                                deadline.duration_since(std::time::SystemTime::now())
                            {
                                std::thread::sleep(duration);
                            }
                        }
                        let now = std::time::Instant::now();
                        let elapsed = now - prev;
                        prev = now;

                        audio_buffer.resize(0, 0);

                        let (produced_video, next_deadline) =
                            media.frame(start, elapsed, &mut audio_buffer, texture.clone())?;

                        if produced_video {
                            // Try and put a frame but if we are being back pressured then dump and run
                            let new_texture = dx::create_texture_sync(
                                &device,
                                width,
                                height,
                                windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12,
                            )?;

                            unsafe { context.CopyResource(&new_texture, &texture) };

                            match h264_control.try_send(encoder::EncoderControl::Frame(
                                new_texture,
                                start
                                    + std::time::Duration::from_nanos(
                                        media.video_timestamp as u64 * 100,
                                    ),
                            )) {
                                Ok(_) => {}
                                Err(_err) => {
                                    log::debug!("video backpressured")
                                }
                            }
                        }

                        if audio_buffer.len() > 0 {
                            log::trace!("produced audio");

                            // Try and put a frame but if we are being back pressured then dump and run
                            match event_tx.try_send(MediaEvent::Audio(audio_buffer.clone())) {
                                Ok(_) => {}
                                Err(_err) => {
                                    log::trace!("audio backpressured");
                                }
                            }
                        }

                        deadline = next_deadline;
                    }

                    eyre::Ok(())
                }
            })
            .await
            .unwrap()
            {
                Ok(_) => log::warn!("media::produce exit Ok"),
                Err(err) => log::error!("media::produce exit err {err} {err:?}"),
            }
        }
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            while let Some(event) = h264_event.recv().await {
                match event {
                    encoder::EncoderEvent::Data(data) => {
                        event_tx.send(MediaEvent::Video(data)).await.unwrap()
                    }
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}
