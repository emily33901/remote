use encoder::FrameIsKeyframe;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_DISABLE_OLE1DDE};

pub mod dx;

pub mod produce;

pub mod decoder;
pub mod encoder;

mod color_conversion;
mod desktop_duplication;
pub(crate) mod file_sink;
mod mf;

use eyre::Result;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VideoBuffer {
    pub data: Vec<u8>,
    pub sequence_header: Option<Vec<u8>>,
    pub time: std::time::SystemTime,
    pub duration: std::time::Duration,
    pub key_frame: FrameIsKeyframe,
}

const ARBITRARY_CHANNEL_LIMIT: usize = 10;

pub enum MediaEvent {
    Audio(Vec<u8>),
    Video(VideoBuffer),
}

pub enum MediaControl {}

pub async fn produce(
    path: &str,
    width: u32,
    height: u32,
    target_framerate: u32,
    bitrate: u32,
) -> Result<(mpsc::Sender<MediaControl>, mpsc::Receiver<MediaEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    tokio::spawn(async move {
        while let Some(control) = control_rx.recv().await {
            match control {}
        }
    });

    let (h264_control, mut h264_event) =
        encoder::h264_encoder(width, height, target_framerate, bitrate).await?;

    tokio::spawn({
        let event_tx = event_tx.clone();
        let path = path.to_owned();
        async move {
            match tokio::task::spawn_blocking({
                move || {
                    unsafe {
                        CoInitializeEx(None, COINIT_DISABLE_OLE1DDE)?;
                    }

                    let (device, _context) = dx::create_device()?;

                    let texture =
                        dx::TextureBuilder::new(&device, width, height, dx::TextureFormat::NV12)
                            .keyed_mutex()
                            .nt_handle()
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
                            .nt_handle()
                            .keyed_mutex()
                            .build()?;

                            dx::copy_texture(&new_texture, &texture, None)?;

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
                                    log::debug!("audio backpressured")
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
                Err(err) => log::error!("media::produce exit err {err:?}"),
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

pub async fn duplicate_desktop(
    width: u32,
    height: u32,
    framerate: u32,
    bitrate: u32,
) -> Result<(mpsc::Sender<MediaControl>, mpsc::Receiver<MediaEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

    let (h264_control, mut h264_event) =
        encoder::h264_encoder(width, height, framerate, bitrate).await?;

    let (convert_control, mut convert_event) = color_conversion::converter(
        width,
        height,
        framerate,
        color_conversion::Format::BGRA,
        color_conversion::Format::NV12,
    )
    .await?;

    let (_dd_control, mut dd_event) = desktop_duplication::desktop_duplication()?;

    tokio::spawn(async move { while let Some(_control) = control_rx.recv().await {} });

    tokio::spawn(async move {
        match async move {
            while let Some(event) = dd_event.recv().await {
                match event {
                    desktop_duplication::DDEvent::Size(_, _) => {}
                    desktop_duplication::DDEvent::Frame(texture, time) => {
                        convert_control
                            .send(color_conversion::ConvertControl::Frame(texture, time))
                            .await?
                    }
                }
            }
            eyre::Ok(())
        }
        .await
        {
            Ok(_) => {}
            Err(err) => log::error!("dd event err {err}"),
        }
    });

    tokio::spawn(async move {
        match async move {
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
        }
        .await
        {
            Ok(_) => {}
            Err(err) => log::error!("convert event err {err}"),
        }
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            match async move {
                while let Some(event) = h264_event.recv().await {
                    match event {
                        encoder::EncoderEvent::Data(data) => {
                            event_tx.send(MediaEvent::Video(data)).await?
                        }
                    }
                }

                eyre::Ok(())
            }
            .await
            {
                Ok(_) => {}
                Err(err) => log::error!("encoder event err {err}"),
            }
        }
    });

    Ok((control_tx, event_rx))
}
