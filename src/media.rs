use tokio::sync::mpsc;
use windows::Win32::{
    Graphics::Direct3D11::{
        ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_MAPPED_SUBRESOURCE,
        D3D11_MAP_READ,
    },
    System::Com::{CoInitializeEx, COINIT_DISABLE_OLE1DDE},
};

mod dx {
    use eyre::{eyre, Result};
    use windows::{
        core::{ComInterface, IUnknown},
        Win32::Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Dxgi::{
                Common::DXGI_FORMAT, IDXGIAdapter1, IDXGIFactory4, DXGI_ADAPTER_FLAG,
                DXGI_ADAPTER_FLAG_NONE, DXGI_ADAPTER_FLAG_SOFTWARE,
            },
        },
    };

    const FEATURE_LEVELS: [D3D_FEATURE_LEVEL; 9] = [
        D3D_FEATURE_LEVEL_12_1,
        D3D_FEATURE_LEVEL_12_0,
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
        D3D_FEATURE_LEVEL_9_3,
        D3D_FEATURE_LEVEL_9_2,
        D3D_FEATURE_LEVEL_9_1,
    ];

    const FLAGS: D3D11_CREATE_DEVICE_FLAG = D3D11_CREATE_DEVICE_FLAG(
        D3D11_CREATE_DEVICE_BGRA_SUPPORT.0 as u32 | D3D11_CREATE_DEVICE_VIDEO_SUPPORT.0 as u32,
    );

    pub(crate) fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;

            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                None,
                FLAGS,
                Some(&FEATURE_LEVELS),
                D3D11_SDK_VERSION,
                Some(&mut device as *mut _),
                None,
                Some(&mut context as *mut _),
            )?;

            let device = device.unwrap();
            let multithreaded_device: ID3D11Multithread = device.cast()?;
            multithreaded_device.SetMultithreadProtected(true);

            Ok((device, context.unwrap()))
        }
    }

    pub(crate) fn create_texture(
        device: &ID3D11Device,
        w: u32,
        h: u32,
        format: DXGI_FORMAT,
    ) -> Result<ID3D11Texture2D> {
        let description = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 1,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32
                | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 as u32,
        };

        let mut texture: Option<ID3D11Texture2D> = None;

        unsafe {
            device.CreateTexture2D(&description as *const _, None, Some(&mut texture as *mut _))?
        };

        texture.ok_or(eyre!("Unable to create texture"))
    }

    pub(crate) fn create_staging_texture(
        device: &ID3D11Device,
        w: u32,
        h: u32,
        format: DXGI_FORMAT,
    ) -> Result<ID3D11Texture2D> {
        let description = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 0,
            ArraySize: 1,
            Format: format,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };

        let mut texture: Option<ID3D11Texture2D> = None;

        unsafe {
            device.CreateTexture2D(&description as *const _, None, Some(&mut texture as *mut _))?
        };

        texture.ok_or(eyre!("Unable to create texture"))
    }
}

mod mf {
    use std::mem::{ManuallyDrop, MaybeUninit};

    use eyre::{eyre, Result};
    use windows::{
        core::{ComInterface, IUnknown, HSTRING},
        Win32::{
            Graphics::{
                Direct3D11::{
                    ID3D11Device, ID3D11Texture2D, D3D11_BOX, D3D11_RESOURCE_MISC_FLAG,
                    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_TEXTURE2D_DESC,
                },
                Dxgi::IDXGIKeyedMutex,
            },
            Media::MediaFoundation::*,
            System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
        },
    };

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

        let mut reset_token = 0_u32;
        let mut device_manager: Option<IMFDXGIDeviceManager> = None;

        unsafe {
            MFCreateDXGIDeviceManager(&mut reset_token as *mut _, &mut device_manager as *mut _)
        }?;

        let device_manager = device_manager.unwrap();

        unsafe { device_manager.ResetDevice(device, reset_token) }?;

        let mut source_attributes: Option<IMFAttributes> = None;

        unsafe { MFCreateAttributes(&mut source_attributes as *mut _, 2) }?;

        let source_attributes = source_attributes.unwrap();

        unsafe {
            source_attributes
                .SetUnknown(&MF_SOURCE_READER_D3D_MANAGER as *const _, &device_manager)?;
            source_attributes.SetUINT32(&MF_SA_D3D11_SHARED_WITHOUT_MUTEX as *const _, 1)?;

            source_attributes.SetUINT32(
                &MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING as *const _,
                1,
            )?;
            source_attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS as *const _, 1)?;
            source_attributes.SetGUID(
                &MF_MT_SUBTYPE as *const _,
                &MFVideoFormat_ARGB32 as *const _,
            )?;
        }

        // let mut source_reader: Option<IMFSourceReader> = None;

        let source_reader =
            unsafe { MFCreateSourceReaderFromMediaSource(&media_source, &source_attributes) }?;

        let video_type = unsafe { MFCreateMediaType() }?;
        unsafe {
            video_type.SetGUID(
                &MF_MT_MAJOR_TYPE as *const _,
                &MFMediaType_Video as *const _,
            )?;
            video_type.SetGUID(
                &MF_MT_SUBTYPE as *const _,
                &MFVideoFormat_ARGB32 as *const _,
            )?;

            let width_height = (width as u64) << 32 | (height as u64);

            video_type.SetUINT64(&MF_MT_FRAME_SIZE, width_height)?;
        }

        unsafe {
            source_reader.SetCurrentMediaType(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                None,
                &video_type,
            )
        }?;

        let audio_type = unsafe { MFCreateMediaType() }?;

        unsafe {
            audio_type.SetGUID(
                &MF_MT_MAJOR_TYPE as *const _,
                &MFMediaType_Audio as *const _,
            )?;
            audio_type.SetGUID(&MF_MT_SUBTYPE as *const _, &MFAudioFormat_PCM as *const _)?;
        }

        unsafe {
            source_reader.SetCurrentMediaType(
                MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                None,
                &audio_type,
            )
        }?;

        let audio_type = unsafe {
            source_reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
        }?;

        // Set up resamplers' output type
        let resampler_output_type = unsafe { MFCreateMediaType() }?;
        unsafe {
            resampler_output_type.SetGUID(
                &MF_MT_MAJOR_TYPE as *const _,
                &MFMediaType_Audio as *const _,
            )?;
            resampler_output_type
                .SetGUID(&MF_MT_SUBTYPE as *const _, &MFAudioFormat_PCM as *const _)?;
            resampler_output_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS as *const _, 2)?;
            resampler_output_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND as *const _, 44100)?;
            resampler_output_type.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT as *const _, 4)?;
            resampler_output_type
                .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND as *const _, 44100 * 4)?;
            resampler_output_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE as *const _, 16)?;
            resampler_output_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT as *const _, 1)?;
        }

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
        video_timestamp: i64,
        audio_timestamp: i64,
    }

    impl Media {
        pub(crate) fn new(
            device: &ID3D11Device,
            path: &str,
            width: u32,
            height: u32,
        ) -> Result<Self> {
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
                let native_type = unsafe {
                    self.source_reader.GetNativeMediaType(
                        MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        MF_SOURCE_READER_CURRENT_TYPE_INDEX.0 as u32,
                    )
                }?;

                let first_output = unsafe {
                    self.source_reader
                        .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                }?;

                let mut width = 0;
                let mut height = 0;

                let width_height =
                    unsafe { first_output.GetUINT64(&MF_MT_FRAME_SIZE as *const _) }?;

                let width = width_height >> 32 & 0xFFFF_FFFF;
                let height = width_height & 0xFFFF_FFFF;

                let fps_num_denom = unsafe { native_type.GetUINT64(&MF_MT_FRAME_RATE) }?;
                let numerator = fps_num_denom >> 32 & 0xFFFF_FFFF;
                let denominator = fps_num_denom & 0xFFFF_FFFF;
                let fps = (numerator as f32) / (denominator as f32);

                log::debug!("Media::debug_media_format: VO: {width}x{height} @ {fps} fps");
            }

            let debug_audio_type = |output: IMFMediaType, typ: &str| -> eyre::Result<()> {
                let channels = unsafe { output.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS as *const _) }?;
                let samples_per_sec =
                    unsafe { output.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND as *const _) }?;
                let bits_per_sample =
                    unsafe { output.GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE as *const _) }?;
                let block_align =
                    unsafe { output.GetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT as *const _) }?;
                let avg_per_sec =
                    unsafe { output.GetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND as *const _) }?;

                log::debug!("Media::debug_media_format: {typ}: c:{channels} sps:{samples_per_sec} bps:{bits_per_sample} ba:{block_align} aps:{avg_per_sec}");

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

                    let mut texture: MaybeUninit<ID3D11Texture2D> = MaybeUninit::uninit();

                    unsafe {
                        dxgi_buffer.GetResource(
                            &ID3D11Texture2D::IID as *const _,
                            &mut texture as *mut _ as *mut *mut std::ffi::c_void,
                        )
                    }?;

                    let mut subresource_index = unsafe { dxgi_buffer.GetSubresourceIndex()? };
                    let texture = unsafe { texture.assume_init() };

                    let mut in_desc = D3D11_TEXTURE2D_DESC::default();
                    let mut out_desc = D3D11_TEXTURE2D_DESC::default();
                    unsafe {
                        texture.GetDesc(&mut in_desc as *mut _);
                        output.GetDesc(&mut out_desc as *mut _);
                    }

                    // If keyed mutex then lock keyed muticies

                    let keyed_in = if D3D11_RESOURCE_MISC_FLAG(in_desc.MiscFlags as i32)
                        .contains(D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX)
                    {
                        let keyed: IDXGIKeyedMutex = texture.cast()?;
                        unsafe {
                            keyed.AcquireSync(0, u32::MAX)?;
                        }
                        Some(keyed)
                    } else {
                        None
                    };

                    let keyed_out = if D3D11_RESOURCE_MISC_FLAG(in_desc.MiscFlags as i32)
                        .contains(D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX)
                    {
                        let keyed: IDXGIKeyedMutex = texture.cast()?;
                        unsafe {
                            keyed.AcquireSync(0, u32::MAX)?;
                        }
                        Some(keyed)
                    } else {
                        None
                    };

                    let device = unsafe { texture.GetDevice() }?;
                    let context = unsafe { device.GetImmediateContext() }?;

                    let region = D3D11_BOX {
                        left: 0,
                        top: 0,
                        front: 0,
                        right: out_desc.Width,
                        bottom: out_desc.Height,
                        back: 1,
                    };
                    unsafe {
                        context.CopySubresourceRegion(
                            &output,
                            0,
                            0,
                            0,
                            0,
                            &texture,
                            subresource_index,
                            Some(&region),
                        )
                    };

                    if let Some(keyed) = keyed_out {
                        unsafe {
                            keyed.ReleaseSync(0)?;
                        }
                    }

                    if let Some(keyed) = keyed_in {
                        unsafe {
                            keyed.ReleaseSync(0)?;
                        }
                    }

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

            let buffer = unsafe { MFCreateMemoryBuffer(1024) }?;
            unsafe { sample.AddBuffer(&buffer) }?;

            let mut output_buffer = MFT_OUTPUT_DATA_BUFFER::default();
            output_buffer.pSample = ManuallyDrop::new(Some(sample.clone()));
            output_buffer.dwStatus = 0;
            output_buffer.dwStreamID = 0;

            let mut status = 0_u32;
            unsafe {
                self.resampler
                    .ProcessOutput(0, &mut [output_buffer], &mut status)
            }?;

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
            elapsed: std::time::Duration,
            output_audio: &mut Vec<u8>,
            output_texture: ID3D11Texture2D,
        ) -> Result<(bool, Option<std::time::SystemTime>)> {
            self.cur_time += elapsed.as_nanos() as i64 / 100;

            let produced_video = self.video_frame(self.cur_time, output_texture)?;
            self.audio_frame(self.cur_time, output_audio)?;

            let next_deadline = std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_nanos(self.audio_timestamp as u64 * 100).min(
                    std::time::Duration::from_nanos(self.video_timestamp as u64 * 100),
                );

            Ok((produced_video, Some(next_deadline)))
        }
    }
}

use eyre::{eyre, Result};

use crate::util;

pub(crate) enum MediaEvent {
    Audio(Vec<u8>),
    Video(Vec<u8>),
}

pub(crate) enum MediaControl {}

pub(crate) async fn produce(
    path: &str,
    width: u32,
    height: u32,
) -> Result<(mpsc::Sender<MediaControl>, mpsc::Receiver<MediaEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(10);
    let (control_tx, mut control_rx) = mpsc::channel(10);

    tokio::spawn(async move {
        while let Some(control) = control_rx.recv().await {
            match control {}
        }
    });
    tokio::spawn({
        let path = path.to_owned();
        async move {
            match tokio::task::spawn_blocking({
                move || {
                    unsafe {
                        CoInitializeEx(None, COINIT_DISABLE_OLE1DDE)?;
                    }

                    let (device, context) = dx::create_device()?;

                    let texture = dx::create_staging_texture(
                        &device,
                        width,
                        height,
                        windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                    )?;

                    let path = path.to_owned();
                    let mut media = mf::Media::new(&device, &path, width, height)?;

                    media.debug_media_format()?;

                    let mut deadline: Option<std::time::SystemTime> = None;
                    let mut prev = std::time::Instant::now();

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
                            media.frame(elapsed, &mut audio_buffer, texture.clone())?;

                        if produced_video {
                            // Make video into buffer
                            log::trace!("produced video");
                            let mut mapped_resource = D3D11_MAPPED_SUBRESOURCE::default();
                            unsafe {
                                context.Map(
                                    &texture,
                                    0,
                                    D3D11_MAP_READ,
                                    0,
                                    Some(&mut mapped_resource as *mut _),
                                )
                            }?;

                            let mut buffer = vec![];
                            buffer.resize((width * height * 4) as usize, 0_u8);

                            unsafe {
                                std::ptr::copy(
                                    mapped_resource.pData as *mut u8,
                                    buffer.as_mut_ptr(),
                                    buffer.len(),
                                );
                            }

                            unsafe { context.Unmap(&texture, 0) };

                            futures::executor::block_on(tokio::spawn({
                                let event_tx = event_tx.clone();
                                async move {
                                    util::send(
                                        "media producer video to media event",
                                        &event_tx,
                                        MediaEvent::Video(buffer),
                                    )
                                    .await
                                    .unwrap();
                                }
                            }))
                            .unwrap();
                        }

                        if audio_buffer.len() > 0 {
                            log::trace!("produced audio");

                            futures::executor::block_on(tokio::spawn({
                                let audio_buffer = audio_buffer.clone();
                                let event_tx = event_tx.clone();
                                async move {
                                    util::send(
                                        "media producer audio to media event",
                                        &event_tx,
                                        MediaEvent::Audio(audio_buffer),
                                    )
                                    .await
                                    .unwrap();
                                }
                            }))
                            .unwrap();
                        }

                        deadline = next_deadline;
                    }

                    eyre::Ok(())
                }
            })
            .await
            .unwrap()
            {
                Ok(_) => log::debug!("media::produce exit Ok"),
                Err(err) => log::debug!("media::produce exit err {err} {err:?}"),
            }
        }
    });

    Ok((control_tx, event_rx))
}
