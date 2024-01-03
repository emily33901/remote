use tokio::sync::mpsc;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_DISABLE_OLE1DDE};

pub(crate) mod dx;

mod produce;

pub(crate) mod decoder;
pub(crate) mod encoder;

mod color_conversion;
mod desktop_duplication;
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

                    let texture =
                        dx::TextureBuilder::new(&device, width, height, dx::TextureFormat::NV12)
                            .build()?;

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
                            let new_texture = dx::TextureBuilder::new(
                                &device,
                                width,
                                height,
                                dx::TextureFormat::NV12,
                            )
                            .build()?;

                            unsafe { context.CopyResource(&new_texture, &texture) };

                            match h264_control.try_send(encoder::EncoderControl::Frame(
                                new_texture,
                                start
                                    + std::time::Duration::from_nanos(
                                        media.video_timestamp as u64 * 100,
                                    ),
                            )) {
                                Ok(_) => {}
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    log::debug!("video backpressured")
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    log::error!("produce channel closed, going down");
                                    break;
                                }
                            }
                        }

                        if audio_buffer.len() > 0 {
                            log::trace!("produced audio");

                            // Try and put a frame but if we are being back pressured then dump and run
                            match event_tx.try_send(MediaEvent::Audio(audio_buffer.clone())) {
                                Ok(_) => {}
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    log::debug!("video backpressured")
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    log::error!("produce channel closed, going down");
                                    break;
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
            match tokio::spawn(async move {
                while let Some(event) = h264_event.recv().await {
                    match event {
                        encoder::EncoderEvent::Data(data) => {
                            event_tx.send(MediaEvent::Video(data)).await?
                        }
                    }
                }

                eyre::Ok(())
            })
            .await
            .unwrap()
            {
                Ok(_) => {}
                Err(err) => log::error!("encoder event err {err}"),
            }
        }
    });

    Ok((control_tx, event_rx))
}

pub(crate) async fn duplicate_desktop(
    width: u32,
    height: u32,
    bitrate: u32,
) -> Result<(mpsc::Sender<MediaControl>, mpsc::Receiver<MediaEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (h264_control, mut h264_event) = encoder::h264_encoder(width, height, 30, bitrate).await?;

    // let (convert_control, mut convert_event) =
    //     color_conversion::convert_bgra_to_nv12(width, height).await?;

    let (convert_control, mut convert_event) = color_conversion::converter(
        width,
        height,
        color_conversion::Format::BGRA,
        color_conversion::Format::NV12,
    )
    .await?;

    let (dd_control, mut dd_event) = desktop_duplication::desktop_duplication()?;

    tokio::spawn(async move { while let Some(control) = control_rx.recv().await {} });

    tokio::spawn(async move {
        while let Some(event) = dd_event.recv().await {
            match event {
                desktop_duplication::DDEvent::Size(_, _) => {}
                desktop_duplication::DDEvent::Frame(texture, time) => {
                    let _ = convert_control
                        .send(color_conversion::ConvertControl::Frame(texture, time))
                        .await;
                    //         .unwrap();
                }
            }
        }
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            match tokio::spawn(async move {
                while let Some(event) = h264_event.recv().await {
                    match event {
                        encoder::EncoderEvent::Data(data) => {
                            event_tx.send(MediaEvent::Video(data)).await?
                        }
                    }
                }

                eyre::Ok(())
            })
            .await
            .unwrap()
            {
                Ok(_) => {}
                Err(err) => log::error!("encoder event err {err}"),
            }
        }
    });

    tokio::spawn(async move {
        match tokio::spawn(async move {
            while let Some(event) = convert_event.recv().await {
                match event {
                    color_conversion::ConvertEvent::Frame(frame, time) => {
                        h264_control
                            .send(encoder::EncoderControl::Frame(frame, time))
                            .await?
                    }
                }
            }
            eyre::Ok(())
        })
        .await
        .unwrap()
        {
            Ok(_) => {}
            Err(err) => log::error!("convert event err {err}"),
        }
    });

    Ok((control_tx, event_rx))
}
