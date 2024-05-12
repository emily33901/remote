use std::mem::{ManuallyDrop, MaybeUninit};

use eyre::Result;
use tokio::sync::mpsc;
use windows::{
    core::{IUnknown, Interface, HSTRING},
    Win32::{
        Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D},
        Media::MediaFoundation::*,
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
    },
};

use crate::{
    dx,
    encoder::{self, Encoder},
    texture_pool::TexturePool,
    Encoding, EncodingOptions, VideoBuffer, ARBITRARY_MEDIA_CHANNEL_LIMIT,
};

use super::mf::{debug_video_format, IMFAttributesExt, IMFDXGIBufferExt};

pub(crate) fn open_media(
    device: &ID3D11Device,
    path: &str,
    width: u32,
    height: u32,
) -> Result<(
    IMFMediaSource,
    IMFSourceReader,
    IMFTransform,
    IMFDXGIDeviceManager,
)> {
    unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)? }

    let source_resolver = unsafe { MFCreateSourceResolver() }?;

    let mut object_type = MF_OBJECT_INVALID;

    let mut media_source_unknown: Option<IUnknown> = None;

    unsafe {
        source_resolver.CreateObjectFromURL(
            &HSTRING::from(path),
            MF_RESOLUTION_MEDIASOURCE.0 as u32,
            None,
            &mut object_type,
            &mut media_source_unknown as *mut _,
        )
    }?;

    let media_source: IMFMediaSource = media_source_unknown.unwrap().cast()?;

    let device_manager = super::mf::create_dxgi_manager(device)?;

    let source_attributes = super::mf::create_attributes()?;

    source_attributes.set_unknown(&MF_SOURCE_READER_D3D_MANAGER, &device_manager)?;
    source_attributes.set_u32(&MF_SA_D3D11_SHARED_WITHOUT_MUTEX, 1)?;

    source_attributes.set_u32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
    source_attributes.set_u32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
    source_attributes.set_guid(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;

    // let mut source_reader: Option<IMFSourceReader> = None;

    let source_reader =
        unsafe { MFCreateSourceReaderFromMediaSource(&media_source, &source_attributes) }?;

    let video_type = unsafe { MFCreateMediaType() }?;

    video_type.set_guid(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    video_type.set_guid(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;

    video_type.set_fraction(&MF_MT_FRAME_SIZE, width, height)?;
    video_type.set_fraction(&MF_MT_FRAME_RATE, 30, 1)?;

    unsafe {
        source_reader.SetCurrentMediaType(
            MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
            None,
            &video_type,
        )
    }?;

    let audio_type = unsafe { MFCreateMediaType() }?;

    audio_type.set_guid(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    audio_type.set_guid(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;

    unsafe {
        source_reader.SetCurrentMediaType(
            MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
            None,
            &audio_type,
        )
    }?;

    let audio_type =
        unsafe { source_reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32) }?;

    // Set up resamplers' output type
    let resampler_output_type = unsafe { MFCreateMediaType() }?;

    resampler_output_type.set_guid(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    resampler_output_type.set_guid(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
    resampler_output_type.set_u32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
    resampler_output_type.set_u32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 44100)?;
    resampler_output_type.set_u32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
    resampler_output_type.set_u32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 44100 * 4)?;
    resampler_output_type.set_u32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    resampler_output_type.set_u32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;

    let resampler: IMFTransform = unsafe {
        CoCreateInstance(
            &CLSID_AudioResamplerMediaObject as *const _,
            None,
            CLSCTX_INPROC_SERVER,
        )
    }?;

    let resampler_props: IWMResamplerProps = resampler.cast()?;
    unsafe { resampler_props.SetHalfFilterLength(60) }?;

    unsafe {
        resampler.SetInputType(0, &audio_type, 0)?;
        resampler.SetOutputType(0, &resampler_output_type, 0)?;
    }

    unsafe {
        resampler.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
        resampler.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        resampler.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
    }

    Ok((media_source, source_reader, resampler, device_manager))
}

pub(crate) struct Media {
    resampler: IMFTransform,
    source_reader: IMFSourceReader,
    device_manager: IMFDXGIDeviceManager,
    media_source: IMFMediaSource,
    cur_time: i64,
    /// Next video timestamp in 100 nanoseconds
    pub(crate) video_timestamp: i64,
    /// Next audio timestamp in 100 nanoseconds
    pub(crate) audio_timestamp: i64,
}

impl Media {
    pub(crate) fn new(device: &ID3D11Device, path: &str, width: u32, height: u32) -> Result<Self> {
        let (media_source, source_reader, resampler, device_manager) =
            open_media(device, path, width, height)?;

        Ok(Self {
            resampler: resampler,
            source_reader: source_reader,
            device_manager: device_manager,
            media_source,
            cur_time: 0,
            video_timestamp: 0,
            audio_timestamp: 0,
        })
    }

    pub(crate) fn debug_media_format(&self) -> Result<()> {
        {
            let _native_type = unsafe {
                self.source_reader.GetNativeMediaType(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    MF_SOURCE_READER_CURRENT_TYPE_INDEX.0 as u32,
                )
            }?;

            let first_output = unsafe {
                self.source_reader
                    .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
            }?;

            debug_video_format(&first_output)?;
        }

        let debug_audio_type = |output: IMFMediaType, typ: &str| -> eyre::Result<()> {
            let channels = output.get_u32(&MF_MT_AUDIO_NUM_CHANNELS)?;
            let samples_per_sec = output.get_u32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?;
            let bits_per_sample = output.get_u32(&MF_MT_AUDIO_BITS_PER_SAMPLE)?;
            let block_align = output.get_u32(&MF_MT_AUDIO_BLOCK_ALIGNMENT)?;
            let avg_per_sec = output.get_u32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND)?;

            tracing::debug!("Media::debug_media_format: {typ}: c:{channels} sps:{samples_per_sec} bps:{bits_per_sample} ba:{block_align} aps:{avg_per_sec}");

            Ok(())
        };

        {
            let first_output = unsafe {
                self.source_reader
                    .GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
            }?;

            debug_audio_type(first_output, "AI")?;
        }

        {
            let native_type = unsafe {
                self.source_reader.GetNativeMediaType(
                    MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                    MF_SOURCE_READER_CURRENT_TYPE_INDEX.0 as u32,
                )
            }?;

            debug_audio_type(native_type, "AO")?;
        }

        {
            let native_type = unsafe { self.resampler.GetInputCurrentType(0 as u32) }?;
            debug_audio_type(native_type, "RI")?;
        }

        {
            let native_type = unsafe { self.resampler.GetOutputCurrentType(0 as u32) }?;
            debug_audio_type(native_type, "RO")?;
        }
        Ok(())
    }

    fn video_frame(&mut self, cur_time: i64, output: ID3D11Texture2D) -> Result<bool> {
        if cur_time < self.video_timestamp {
            return Ok(false);
        }

        let mut flags = 0_u32;
        let mut sample: Option<IMFSample> = None;
        let mut stream_index = 0_u32;

        unsafe {
            self.source_reader.ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                Some(&mut stream_index as *mut _),
                Some(&mut flags as *mut _),
                Some(&mut self.video_timestamp),
                Some(&mut sample as *mut _),
            )?;
        }

        match sample {
            Some(sample) => {
                unsafe { sample.SetSampleTime(self.video_timestamp) }?;

                let media_buffer = unsafe { sample.GetBufferByIndex(0) }?;
                let dxgi_buffer: IMFDXGIBuffer = media_buffer.cast()?;

                let (texture, subresource_index) = dxgi_buffer.texture()?;

                super::dx::copy_texture(&output, &texture, Some(subresource_index))?;

                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn audio_sample(&self) -> Result<Option<(IMFSample, i64)>> {
        let mut flags = 0_u32;
        let mut sample: Option<IMFSample> = None;

        let mut time = 0_i64;

        unsafe {
            self.source_reader.ReadSample(
                MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                0,
                None,
                Some(&mut flags as *mut _),
                Some(&mut time),
                Some(&mut sample as *mut _),
            )?;
        }

        if let Some(sample) = sample {
            unsafe { sample.SetSampleTime(time) }?;
            Ok(Some((sample, time)))
        } else {
            Ok(None)
        }
    }

    fn audio_frame(&mut self, cur_time: i64, output: &mut Vec<u8>) -> Result<()> {
        if cur_time < self.audio_timestamp {
            return Ok(());
        }

        while _MFT_INPUT_STATUS_FLAGS(unsafe { self.resampler.GetInputStatus(0) }? as i32)
            == MFT_INPUT_STATUS_ACCEPT_DATA
        {
            match self.audio_sample()? {
                Some(sample) => unsafe {
                    self.resampler.ProcessInput(0, &sample.0, 0)?;
                },
                None => break,
            }
        }

        let sample = unsafe { MFCreateSample() }?;

        let buffer = unsafe { MFCreateMemoryBuffer(2048) }?;
        unsafe { sample.AddBuffer(&buffer) }?;

        let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();
        output_buffer.pSample = ManuallyDrop::new(Some(sample));
        output_buffer.dwStatus = 0;
        output_buffer.dwStreamID = 0;

        let output_buffers = &mut [output_buffer];

        let mut status = 0_u32;
        unsafe { self.resampler.ProcessOutput(0, output_buffers, &mut status) }?;

        let sample = output_buffers[0].pSample.take().unwrap();

        self.audio_timestamp = unsafe { sample.GetSampleTime()? };

        let media_buffer = unsafe { sample.ConvertToContiguousBuffer() }?;

        unsafe {
            let mut begin: MaybeUninit<*mut u8> = MaybeUninit::uninit();
            let mut len = 0_u32;

            media_buffer.Lock(&mut begin as *mut _ as *mut *mut u8, None, Some(&mut len))?;

            output.resize(len as usize, 0);

            std::ptr::copy(begin.assume_init(), output.as_mut_ptr(), len as usize);

            media_buffer.Unlock()?;
        };

        Ok(())
    }

    pub(crate) fn frame(
        &mut self,
        start_time: std::time::SystemTime,
        elapsed: std::time::Duration,
        output_audio: &mut Vec<u8>,
        output_texture: ID3D11Texture2D,
    ) -> Result<(bool, Option<std::time::SystemTime>)> {
        self.cur_time += elapsed.as_nanos() as i64 / 100;

        let produced_video = self.video_frame(self.cur_time, output_texture)?;
        self.audio_frame(self.cur_time, output_audio)?;

        let next_deadline = start_time
            + std::time::Duration::from_nanos(self.audio_timestamp as u64 * 100).min(
                std::time::Duration::from_nanos(self.video_timestamp as u64 * 100),
            );

        Ok((produced_video, Some(next_deadline)))
    }
}

pub enum MediaEvent {
    Audio(Vec<u8>),
    Video(VideoBuffer),
}

pub enum MediaControl {}

pub async fn produce(
    encoder_api: Encoder,
    encoding: Encoding,
    encoding_options: EncodingOptions,
    path: &str,
    width: u32,
    height: u32,
    frame_rate: u32,
) -> Result<(mpsc::Sender<MediaControl>, mpsc::Receiver<MediaEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);
    let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_MEDIA_CHANNEL_LIMIT);

    tokio::spawn(async move {
        while let Some(control) = control_rx.recv().await {
            match control {}
        }
    });

    let (h264_control, mut h264_event) = encoder_api
        .run(width, height, frame_rate, encoding, encoding_options)
        .await?;

    tokio::task::spawn_blocking({
        let event_tx = event_tx.clone();
        let path = path.to_owned();

        move || {
            crate::mf::init()?;

            let (device, _context) = dx::create_device()?;

            let texture = dx::TextureBuilder::new(&device, width, height, dx::TextureFormat::NV12)
                .keyed_mutex()
                .nt_handle()
                .build()?;

            let path = path.to_owned();
            let mut media = Media::new(&device, &path, width, height)?;

            media.debug_media_format()?;

            let mut deadline: Option<std::time::SystemTime> = None;
            let mut prev = std::time::Instant::now();
            let start = std::time::SystemTime::now();

            let mut audio_buffer: Vec<u8> = vec![];

            let output_texture_pool = TexturePool::new(
                || {
                    dx::TextureBuilder::new(&device, width, height, dx::TextureFormat::NV12)
                        .nt_handle()
                        .keyed_mutex()
                        .build()
                        .unwrap()
                },
                10,
            );

            loop {
                if let Some(deadline) = deadline {
                    if let Ok(duration) = deadline.duration_since(std::time::SystemTime::now()) {
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
                    let output_texture = output_texture_pool.acquire();
                    dx::copy_texture(&output_texture, &texture, None)?;

                    h264_control.blocking_send(encoder::EncoderControl::Frame(
                        output_texture,
                        crate::Timestamp::new_hns(media.video_timestamp),
                        crate::Statistics {
                            ..Default::default()
                        },
                    ))?
                }

                if audio_buffer.len() > 0 {
                    tracing::trace!("produced audio");

                    // Try and put a frame but if we are being back pressured then dump and run
                    match event_tx.try_send(MediaEvent::Audio(audio_buffer.clone())) {
                        Ok(_) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            tracing::debug!("audio backpressured")
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            tracing::error!("produce channel closed, going down");
                            break;
                        }
                    }
                }

                deadline = next_deadline;
            }

            eyre::Ok(())
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
                Err(err) => tracing::error!("encoder event err {err}"),
            }
        }
    });

    Ok((control_tx, event_rx))
}
