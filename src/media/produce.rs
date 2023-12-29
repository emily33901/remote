use std::mem::{ManuallyDrop, MaybeUninit};

use eyre::Result;
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
        source_attributes.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER as *const _, &device_manager)?;
        source_attributes.SetUINT32(&MF_SA_D3D11_SHARED_WITHOUT_MUTEX as *const _, 1)?;

        source_attributes.SetUINT32(
            &MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING as *const _,
            1,
        )?;
        source_attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS as *const _, 1)?;
        source_attributes.SetGUID(&MF_MT_SUBTYPE as *const _, &MFVideoFormat_NV12 as *const _)?;
    }

    // let mut source_reader: Option<IMFSourceReader> = None;

    let source_reader =
        unsafe { MFCreateSourceReaderFromMediaSource(&media_source, &source_attributes) }?;

    let video_type = unsafe { MFCreateMediaType() }?;
    unsafe {
        video_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        video_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;

        let width_height = (width as u64) << 32 | (height as u64);

        video_type.SetUINT64(&MF_MT_FRAME_SIZE, width_height)?;

        let fps = (30 as u64) << 32 | (1_u64);
        video_type.SetUINT64(&MF_MT_FRAME_RATE, fps)?;
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
        audio_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        audio_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
    }

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
    unsafe {
        resampler_output_type.SetGUID(
            &MF_MT_MAJOR_TYPE as *const _,
            &MFMediaType_Audio as *const _,
        )?;
        resampler_output_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        resampler_output_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        resampler_output_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 44100)?;
        resampler_output_type.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
        resampler_output_type.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 44100 * 4)?;
        resampler_output_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        resampler_output_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
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

pub(crate) fn debug_video_format(typ: &IMFMediaType) -> Result<()> {
    let width_height = unsafe { typ.GetUINT64(&MF_MT_FRAME_SIZE as *const _) }?;

    let width = width_height >> 32 & 0xFFFF_FFFF;
    let height = width_height & 0xFFFF_FFFF;

    let fps_num_denom = unsafe { typ.GetUINT64(&MF_MT_FRAME_RATE) }?;
    let numerator = fps_num_denom >> 32 & 0xFFFF_FFFF;
    let denominator = fps_num_denom & 0xFFFF_FFFF;
    let fps = (numerator as f32) / (denominator as f32);

    log::info!("Media::debug_media_format: VO: {width}x{height} @ {fps} fps");

    Ok(())
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

                let subresource_index = unsafe { dxgi_buffer.GetSubresourceIndex()? };
                let texture = unsafe { texture.assume_init() };

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
