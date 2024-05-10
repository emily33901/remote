use std::{borrow::Cow, mem::MaybeUninit};

use windows::{
    core::{IUnknown, Interface, Param, PWSTR},
    Win32::{
        Foundation::FALSE,
        Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D},
        Media::MediaFoundation::{
            IMFAttributes, IMFDXGIBuffer, IMFDXGIDeviceManager, IMFMediaBuffer, IMFMediaType,
            IMFSample, MFCreateAttributes, MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer,
            MFCreateMediaType, MFCreateSample, MFStartup, MFSTARTUP_NOSOCKET, MF_MT_FRAME_RATE,
            MF_MT_FRAME_SIZE, MF_VERSION,
        },
        System::Com::{
            CoInitializeEx, CoTaskMemFree, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED,
        },
    },
};

use eyre::Result;

pub(crate) fn with_locked_media_buffer<F: FnOnce(&mut [u8], &mut usize) -> Result<()>>(
    buffer: &IMFMediaBuffer,
    f: F,
) -> Result<()> {
    unsafe {
        let mut begin: MaybeUninit<*mut u8> = MaybeUninit::uninit();
        let mut len = buffer.GetCurrentLength()?;
        let mut max_len = buffer.GetMaxLength()?;

        buffer.Lock(
            &mut begin as *mut _ as *mut *mut u8,
            Some(&mut max_len),
            Some(&mut len),
        )?;

        scopeguard::defer! { buffer.Unlock().unwrap() };

        let mut len = len as usize;
        let max_len = max_len as usize;

        let mut s = std::slice::from_raw_parts_mut(begin.assume_init(), max_len);
        f(&mut s, &mut len)?;

        buffer.SetCurrentLength(len as u32)?;
    }

    Ok(())
}

pub(crate) fn debug_media_type(typ: &IMFMediaType) -> Result<()> {
    let mut text = "mf::debug_media_type:\n".to_owned();

    for i in 0..unsafe { typ.GetCount()? } {
        let mut guid: windows_core::GUID = windows_core::GUID::zeroed();
        let mut value = windows_core::PROPVARIANT::default();
        if let Ok(_) = unsafe { typ.GetItemByIndex(i, &mut guid, Some(&mut value)) } {
            let vt: PropVariantType =
                unsafe { value.as_raw().Anonymous.Anonymous.vt }.try_into()?;

            if let PropVariantType::VtClsid = vt {
                let value = unsafe {
                    *(value.as_raw().Anonymous.Anonymous.Anonymous.pStorage
                        as *mut windows_core::GUID)
                };

                text.push_str(&format!(
                    "\t {}:{} ({vt:?})\n",
                    mf_guid_to_name(&guid),
                    mf_guid_to_name(&value)
                ));
            } else {
                text.push_str(&format!(
                    "\t {}:{} ({vt:?})\n",
                    mf_guid_to_name(&guid),
                    value
                ));
            }
        }
    }

    tracing::debug!("{}", text);

    Ok(())
}

pub(crate) fn debug_video_format(typ: &IMFMediaType) -> Result<()> {
    debug_media_type(typ)?;

    let width_height = unsafe { typ.GetUINT64(&MF_MT_FRAME_SIZE) }?;

    let width = width_height >> 32 & 0xFFFF_FFFF;
    let height = width_height & 0xFFFF_FFFF;

    let fps_num_denom = unsafe { typ.GetUINT64(&MF_MT_FRAME_RATE) }
        .ok()
        .unwrap_or_default();
    let numerator = fps_num_denom >> 32 & 0xFFFF_FFFF;
    let denominator = fps_num_denom & 0xFFFF_FFFF;
    let fps = (numerator as f32) / (denominator as f32);

    tracing::info!("mf::debug_video_format: {width}x{height} @ {fps} fps");

    Ok(())
}

pub(crate) fn create_dxgi_manager(
    device: &ID3D11Device,
) -> windows::core::Result<IMFDXGIDeviceManager> {
    let mut reset_token = 0_u32;
    let mut device_manager: Option<IMFDXGIDeviceManager> = None;

    unsafe {
        MFCreateDXGIDeviceManager(&mut reset_token as *mut _, &mut device_manager as *mut _)
    }?;

    let device_manager = device_manager.unwrap();

    unsafe { device_manager.ResetDevice(device, reset_token) }?;

    Ok(device_manager)
}

pub(crate) fn create_attributes() -> windows::core::Result<IMFAttributes> {
    let mut attributes: Option<IMFAttributes> = None;

    unsafe { MFCreateAttributes(&mut attributes as *mut _, 2) }?;
    Ok(attributes.unwrap())
}

pub(crate) fn create_media_type() -> windows::core::Result<IMFMediaType> {
    unsafe { MFCreateMediaType() }
}

pub(crate) trait IMFAttributesExt {
    fn get_u32(&self, key: &windows::core::GUID) -> windows::core::Result<u32>;
    fn set_u32(&self, key: &windows::core::GUID, value: u32) -> windows::core::Result<()>;

    fn get_u64(&self, key: &windows::core::GUID) -> windows::core::Result<u64>;
    fn set_u64(&self, key: &windows::core::GUID, value: u64) -> windows::core::Result<()>;

    fn set_fraction(
        &self,
        key: &windows::core::GUID,
        top: u32,
        bottom: u32,
    ) -> windows::core::Result<()>;

    fn get_fraction(&self, key: &windows::core::GUID) -> windows::core::Result<(u32, u32)>;

    fn get_guid(&self, key: &windows::core::GUID) -> windows::core::Result<windows::core::GUID>;
    fn set_guid(
        &self,
        key: &windows::core::GUID,
        value: &windows::core::GUID,
    ) -> windows::core::Result<()>;

    fn get_string(&self, key: &windows::core::GUID) -> windows::core::Result<String>;

    fn set_unknown<T: windows::core::Param<IUnknown>>(
        &self,
        key: &windows::core::GUID,
        value: T,
    ) -> windows::core::Result<()>;
}

impl IMFAttributesExt for IMFAttributes {
    fn get_u32(&self, key: &windows::core::GUID) -> windows::core::Result<u32> {
        unsafe { self.GetUINT32(key) }
    }

    fn set_u32(&self, key: &windows::core::GUID, value: u32) -> windows::core::Result<()> {
        unsafe { self.SetUINT32(key, value) }
    }

    fn get_u64(&self, key: &windows::core::GUID) -> windows::core::Result<u64> {
        unsafe { self.GetUINT64(key) }
    }

    fn set_u64(&self, key: &windows::core::GUID, value: u64) -> windows::core::Result<()> {
        unsafe { self.SetUINT64(key, value) }
    }

    fn get_guid(&self, key: &windows::core::GUID) -> windows::core::Result<windows::core::GUID> {
        unsafe { self.GetGUID(key) }
    }

    fn set_guid(
        &self,
        key: &windows::core::GUID,
        value: &windows::core::GUID,
    ) -> windows::core::Result<()> {
        unsafe { self.SetGUID(key, value) }
    }

    fn set_fraction(
        &self,
        key: &windows::core::GUID,
        top: u32,
        bottom: u32,
    ) -> windows::core::Result<()> {
        let frac = (top as u64) << 32 | (bottom as u64);
        self.set_u64(key, frac)
    }

    fn get_string(&self, key: &windows::core::GUID) -> windows::core::Result<String> {
        let mut str = PWSTR::null();
        let mut len = 0;

        unsafe {
            self.GetAllocatedString(key, &mut str, &mut len)?;
        }

        scopeguard::defer!(unsafe { CoTaskMemFree(Some(str.as_ptr() as *const _)) });

        let slice = unsafe { std::slice::from_raw_parts(str.as_ptr(), len as usize) };

        Ok(String::from_utf16_lossy(slice))
    }

    fn set_unknown<T: windows::core::Param<IUnknown>>(
        &self,
        key: &windows::core::GUID,
        value: T,
    ) -> windows::core::Result<()> {
        unsafe { self.SetUnknown(key, value) }
    }

    fn get_fraction(&self, key: &windows::core::GUID) -> windows::core::Result<(u32, u32)> {
        let frac = unsafe { self.get_u64(key) }?;
        let top = frac >> 32 & 0xFFFF_FFFF;
        let bottom = frac & 0xFFFF_FFFF;

        Ok((top as u32, bottom as u32))
    }
}

// TODO(emily): c+p from above, find some better way of doing this similar to impl<T> IMFAttributesExt for T { }
impl IMFAttributesExt for IMFMediaType {
    fn get_u32(&self, key: &windows::core::GUID) -> windows::core::Result<u32> {
        unsafe { self.GetUINT32(key) }
    }

    fn set_u32(&self, key: &windows::core::GUID, value: u32) -> windows::core::Result<()> {
        unsafe { self.SetUINT32(key, value) }
    }

    fn get_u64(&self, key: &windows::core::GUID) -> windows::core::Result<u64> {
        unsafe { self.GetUINT64(key) }
    }

    fn set_u64(&self, key: &windows::core::GUID, value: u64) -> windows::core::Result<()> {
        unsafe { self.SetUINT64(key, value) }
    }

    fn get_guid(&self, key: &windows::core::GUID) -> windows::core::Result<windows::core::GUID> {
        unsafe { self.GetGUID(key) }
    }

    fn set_guid(
        &self,
        key: &windows::core::GUID,
        value: &windows::core::GUID,
    ) -> windows::core::Result<()> {
        unsafe { self.SetGUID(key, value) }
    }

    fn set_fraction(
        &self,
        key: &windows::core::GUID,
        top: u32,
        bottom: u32,
    ) -> windows::core::Result<()> {
        let frac = (top as u64) << 32 | (bottom as u64);
        self.set_u64(key, frac)
    }

    fn get_string(&self, key: &windows::core::GUID) -> windows::core::Result<String> {
        let mut str = PWSTR::null();
        let mut len = 0;

        unsafe {
            self.GetAllocatedString(key, &mut str, &mut len)?;
        }

        scopeguard::defer!(unsafe { CoTaskMemFree(Some(str.as_ptr() as *const _)) });

        let slice = unsafe { std::slice::from_raw_parts(str.as_ptr(), len as usize) };

        Ok(String::from_utf16_lossy(slice))
    }

    fn set_unknown<T: Param<IUnknown>>(
        &self,
        key: &windows::core::GUID,
        value: T,
    ) -> windows::core::Result<()> {
        unsafe { self.SetUnknown(key, value) }
    }

    fn get_fraction(&self, key: &windows::core::GUID) -> windows::core::Result<(u32, u32)> {
        let frac = unsafe { self.get_u64(key) }?;
        let top = frac >> 32 & 0xFFFF_FFFF;
        let bottom = frac & 0xFFFF_FFFF;

        Ok((top as u32, bottom as u32))
    }
}

pub(crate) trait IMFDXGIBufferExt {
    fn texture(&self) -> windows::core::Result<(ID3D11Texture2D, u32)>;
}

impl IMFDXGIBufferExt for IMFDXGIBuffer {
    fn texture(&self) -> windows::core::Result<(ID3D11Texture2D, u32)> {
        let mut texture: MaybeUninit<ID3D11Texture2D> = MaybeUninit::uninit();

        unsafe {
            self.GetResource(
                &ID3D11Texture2D::IID as *const _,
                &mut texture as *mut _ as *mut *mut std::ffi::c_void,
            )
        }?;

        let subresource_index = unsafe { self.GetSubresourceIndex()? };
        let texture = unsafe { texture.assume_init() };

        Ok((texture, subresource_index))
    }
}

pub(crate) fn make_dxgi_sample(
    texture: &ID3D11Texture2D,
    subresource_index: Option<u32>,
) -> windows::core::Result<IMFSample> {
    // NOTE(emily): AMD MF encoder calls Lock2D on this texture so you CANNOT use MFCreateVideoSampleFromSurface.
    // consider maybe making that a special case.

    let dxgi_buffer = unsafe {
        MFCreateDXGISurfaceBuffer(
            &ID3D11Texture2D::IID,
            texture,
            subresource_index.unwrap_or_default(),
            FALSE,
        )?
    };

    let sample = unsafe { MFCreateSample() }?;
    unsafe { sample.AddBuffer(&dxgi_buffer)? };
    Ok(sample)
}

pub(crate) fn init() -> Result<()> {
    unsafe { CoInitializeEx(None, COINIT_DISABLE_OLE1DDE | COINIT_MULTITHREADED) }.ok()?;
    unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)? }

    Ok(())
}

pub(crate) fn mf_guid_to_name(guid: &windows_core::GUID) -> Cow<'static, str> {
    use ::windows::Win32::Media::MediaFoundation::*;
    macro_rules! return_if_matches {
        ($i:expr) => {
            if $i == *guid {
                return stringify!($i).into();
            }
        };
    }

    return_if_matches!(MFASFINDEXER_TYPE_TIMECODE);
    return_if_matches!(MFASFMutexType_Bitrate);
    return_if_matches!(MFASFMutexType_Language);
    return_if_matches!(MFASFMutexType_Presentation);
    return_if_matches!(MFASFMutexType_Unknown);
    return_if_matches!(MFASFSPLITTER_PACKET_BOUNDARY);
    return_if_matches!(MFASFSampleExtension_ContentType);
    return_if_matches!(MFASFSampleExtension_Encryption_KeyID);
    return_if_matches!(MFASFSampleExtension_Encryption_SampleID);
    return_if_matches!(MFASFSampleExtension_FileName);
    return_if_matches!(MFASFSampleExtension_OutputCleanPoint);
    return_if_matches!(MFASFSampleExtension_PixelAspectRatio);
    return_if_matches!(MFASFSampleExtension_SMPTE);
    return_if_matches!(MFASFSampleExtension_SampleDuration);
    return_if_matches!(MFAudioFormat_AAC);
    return_if_matches!(MFAudioFormat_AAC_HDCP);
    return_if_matches!(MFAudioFormat_ADTS);
    return_if_matches!(MFAudioFormat_ADTS_HDCP);
    return_if_matches!(MFAudioFormat_ALAC);
    return_if_matches!(MFAudioFormat_AMR_NB);
    return_if_matches!(MFAudioFormat_AMR_WB);
    return_if_matches!(MFAudioFormat_AMR_WP);
    return_if_matches!(MFAudioFormat_Base);
    return_if_matches!(MFAudioFormat_Base_HDCP);
    return_if_matches!(MFAudioFormat_DRM);
    return_if_matches!(MFAudioFormat_DTS);
    return_if_matches!(MFAudioFormat_DTS_HD);
    return_if_matches!(MFAudioFormat_DTS_LBR);
    return_if_matches!(MFAudioFormat_DTS_RAW);
    return_if_matches!(MFAudioFormat_DTS_UHD);
    return_if_matches!(MFAudioFormat_DTS_UHDY);
    return_if_matches!(MFAudioFormat_DTS_XLL);
    return_if_matches!(MFAudioFormat_Dolby_AC3);
    return_if_matches!(MFAudioFormat_Dolby_AC3_HDCP);
    return_if_matches!(MFAudioFormat_Dolby_AC3_SPDIF);
    return_if_matches!(MFAudioFormat_Dolby_AC4);
    return_if_matches!(MFAudioFormat_Dolby_AC4_V1);
    return_if_matches!(MFAudioFormat_Dolby_AC4_V1_ES);
    return_if_matches!(MFAudioFormat_Dolby_AC4_V2);
    return_if_matches!(MFAudioFormat_Dolby_AC4_V2_ES);
    return_if_matches!(MFAudioFormat_Dolby_DDPlus);
    return_if_matches!(MFAudioFormat_FLAC);
    return_if_matches!(MFAudioFormat_Float);
    return_if_matches!(MFAudioFormat_Float_SpatialObjects);
    return_if_matches!(MFAudioFormat_LPCM);
    return_if_matches!(MFAudioFormat_MP3);
    return_if_matches!(MFAudioFormat_MPEG);
    return_if_matches!(MFAudioFormat_MSP1);
    return_if_matches!(MFAudioFormat_Opus);
    return_if_matches!(MFAudioFormat_PCM);
    return_if_matches!(MFAudioFormat_PCM_HDCP);
    return_if_matches!(MFAudioFormat_Vorbis);
    return_if_matches!(MFAudioFormat_WMASPDIF);
    return_if_matches!(MFAudioFormat_WMAudioV8);
    return_if_matches!(MFAudioFormat_WMAudioV9);
    return_if_matches!(MFAudioFormat_WMAudio_Lossless);
    return_if_matches!(MFCONNECTOR_AGP);
    return_if_matches!(MFCONNECTOR_COMPONENT);
    return_if_matches!(MFCONNECTOR_COMPOSITE);
    return_if_matches!(MFCONNECTOR_DISPLAYPORT_EMBEDDED);
    return_if_matches!(MFCONNECTOR_DISPLAYPORT_EXTERNAL);
    return_if_matches!(MFCONNECTOR_DVI);
    return_if_matches!(MFCONNECTOR_D_JPN);
    return_if_matches!(MFCONNECTOR_HDMI);
    return_if_matches!(MFCONNECTOR_LVDS);
    return_if_matches!(MFCONNECTOR_MIRACAST);
    return_if_matches!(MFCONNECTOR_PCI);
    return_if_matches!(MFCONNECTOR_PCIX);
    return_if_matches!(MFCONNECTOR_PCI_Express);
    return_if_matches!(MFCONNECTOR_SDI);
    return_if_matches!(MFCONNECTOR_SPDIF);
    return_if_matches!(MFCONNECTOR_SVIDEO);
    return_if_matches!(MFCONNECTOR_TRANSPORT_AGNOSTIC_DIGITAL_MODE_A);
    return_if_matches!(MFCONNECTOR_TRANSPORT_AGNOSTIC_DIGITAL_MODE_B);
    return_if_matches!(MFCONNECTOR_UDI_EMBEDDED);
    return_if_matches!(MFCONNECTOR_UDI_EXTERNAL);
    return_if_matches!(MFCONNECTOR_UNKNOWN);
    return_if_matches!(MFCONNECTOR_VGA);
    return_if_matches!(MFENABLETYPE_MF_RebootRequired);
    return_if_matches!(MFENABLETYPE_MF_UpdateRevocationInformation);
    return_if_matches!(MFENABLETYPE_MF_UpdateUntrustedComponent);
    return_if_matches!(MFENABLETYPE_WMDRMV1_LicenseAcquisition);
    return_if_matches!(MFENABLETYPE_WMDRMV7_Individualization);
    return_if_matches!(MFENABLETYPE_WMDRMV7_LicenseAcquisition);
    return_if_matches!(MFImageFormat_JPEG);
    return_if_matches!(MFImageFormat_RGB32);
    return_if_matches!(MFMPEG4Format_Base);
    return_if_matches!(MFMediaType_Audio);
    return_if_matches!(MFMediaType_Binary);
    return_if_matches!(MFMediaType_Default);
    return_if_matches!(MFMediaType_FileTransfer);
    return_if_matches!(MFMediaType_HTML);
    return_if_matches!(MFMediaType_Image);
    return_if_matches!(MFMediaType_Metadata);
    return_if_matches!(MFMediaType_MultiplexedFrames);
    return_if_matches!(MFMediaType_Perception);
    return_if_matches!(MFMediaType_Protected);
    return_if_matches!(MFMediaType_SAMI);
    return_if_matches!(MFMediaType_Script);
    return_if_matches!(MFMediaType_Stream);
    return_if_matches!(MFMediaType_Subtitle);
    return_if_matches!(MFMediaType_Video);
    return_if_matches!(MFNETSOURCE_ACCELERATEDSTREAMINGDURATION);
    return_if_matches!(MFNETSOURCE_AUTORECONNECTLIMIT);
    return_if_matches!(MFNETSOURCE_AUTORECONNECTPROGRESS);
    return_if_matches!(MFNETSOURCE_BROWSERUSERAGENT);
    return_if_matches!(MFNETSOURCE_BROWSERWEBPAGE);
    return_if_matches!(MFNETSOURCE_BUFFERINGTIME);
    return_if_matches!(MFNETSOURCE_CACHEENABLED);
    return_if_matches!(MFNETSOURCE_CLIENTGUID);
    return_if_matches!(MFNETSOURCE_CONNECTIONBANDWIDTH);
    return_if_matches!(MFNETSOURCE_CREDENTIAL_MANAGER);
    return_if_matches!(MFNETSOURCE_CROSS_ORIGIN_SUPPORT);
    return_if_matches!(MFNETSOURCE_DRMNET_LICENSE_REPRESENTATION);
    return_if_matches!(MFNETSOURCE_ENABLE_DOWNLOAD);
    return_if_matches!(MFNETSOURCE_ENABLE_HTTP);
    return_if_matches!(MFNETSOURCE_ENABLE_MSB);
    return_if_matches!(MFNETSOURCE_ENABLE_PRIVATEMODE);
    return_if_matches!(MFNETSOURCE_ENABLE_RTSP);
    return_if_matches!(MFNETSOURCE_ENABLE_STREAMING);
    return_if_matches!(MFNETSOURCE_ENABLE_TCP);
    return_if_matches!(MFNETSOURCE_ENABLE_UDP);
    return_if_matches!(MFNETSOURCE_FRIENDLYNAME);
    return_if_matches!(MFNETSOURCE_HOSTEXE);
    return_if_matches!(MFNETSOURCE_HOSTVERSION);
    return_if_matches!(MFNETSOURCE_HTTP_DOWNLOAD_SESSION_PROVIDER);
    return_if_matches!(MFNETSOURCE_LOGPARAMS);
    return_if_matches!(MFNETSOURCE_LOGURL);
    return_if_matches!(MFNETSOURCE_MAXBUFFERTIMEMS);
    return_if_matches!(MFNETSOURCE_MAXUDPACCELERATEDSTREAMINGDURATION);
    return_if_matches!(MFNETSOURCE_PEERMANAGER);
    return_if_matches!(MFNETSOURCE_PLAYERID);
    return_if_matches!(MFNETSOURCE_PLAYERUSERAGENT);
    return_if_matches!(MFNETSOURCE_PLAYERVERSION);
    return_if_matches!(MFNETSOURCE_PPBANDWIDTH);
    return_if_matches!(MFNETSOURCE_PREVIEWMODEENABLED);
    return_if_matches!(MFNETSOURCE_PROTOCOL);
    return_if_matches!(MFNETSOURCE_PROXYBYPASSFORLOCAL);
    return_if_matches!(MFNETSOURCE_PROXYEXCEPTIONLIST);
    return_if_matches!(MFNETSOURCE_PROXYHOSTNAME);
    return_if_matches!(MFNETSOURCE_PROXYINFO);
    return_if_matches!(MFNETSOURCE_PROXYLOCATORFACTORY);
    return_if_matches!(MFNETSOURCE_PROXYPORT);
    return_if_matches!(MFNETSOURCE_PROXYRERUNAUTODETECTION);
    return_if_matches!(MFNETSOURCE_PROXYSETTINGS);
    return_if_matches!(MFNETSOURCE_RESENDSENABLED);
    return_if_matches!(MFNETSOURCE_RESOURCE_FILTER);
    return_if_matches!(MFNETSOURCE_SSLCERTIFICATE_MANAGER);
    return_if_matches!(MFNETSOURCE_STATISTICS);
    return_if_matches!(MFNETSOURCE_STATISTICS_SERVICE);
    return_if_matches!(MFNETSOURCE_STREAM_LANGUAGE);
    return_if_matches!(MFNETSOURCE_THINNINGENABLED);
    return_if_matches!(MFNETSOURCE_TRANSPORT);
    return_if_matches!(MFNETSOURCE_UDP_PORT_RANGE);
    return_if_matches!(MFNET_SAVEJOB_SERVICE);
    return_if_matches!(MFPROTECTIONATTRIBUTE_BEST_EFFORT);
    return_if_matches!(MFPROTECTIONATTRIBUTE_CONSTRICTVIDEO_IMAGESIZE);
    return_if_matches!(MFPROTECTIONATTRIBUTE_FAIL_OVER);
    return_if_matches!(MFPROTECTIONATTRIBUTE_HDCP_SRM);
    return_if_matches!(MFPROTECTION_ACP);
    return_if_matches!(MFPROTECTION_CGMSA);
    return_if_matches!(MFPROTECTION_CONSTRICTAUDIO);
    return_if_matches!(MFPROTECTION_CONSTRICTVIDEO);
    return_if_matches!(MFPROTECTION_CONSTRICTVIDEO_NOOPM);
    return_if_matches!(MFPROTECTION_DISABLE);
    return_if_matches!(MFPROTECTION_DISABLE_SCREEN_SCRAPE);
    return_if_matches!(MFPROTECTION_FFT);
    return_if_matches!(MFPROTECTION_GRAPHICS_TRANSFER_AES_ENCRYPTION);
    return_if_matches!(MFPROTECTION_HARDWARE);
    return_if_matches!(MFPROTECTION_HDCP);
    return_if_matches!(MFPROTECTION_HDCP_WITH_TYPE_ENFORCEMENT);
    return_if_matches!(MFPROTECTION_PROTECTED_SURFACE);
    return_if_matches!(MFPROTECTION_TRUSTEDAUDIODRIVERS);
    return_if_matches!(MFPROTECTION_VIDEO_FRAMES);
    return_if_matches!(MFPROTECTION_WMDRMOTA);
    return_if_matches!(MFP_POSITIONTYPE_100NS);
    return_if_matches!(MFSampleExtension_3DVideo);
    return_if_matches!(MFSampleExtension_3DVideo_SampleFormat);
    return_if_matches!(MFSampleExtension_AccumulatedNonRefPicPercent);
    return_if_matches!(MFSampleExtension_BottomFieldFirst);
    return_if_matches!(MFSampleExtension_CameraExtrinsics);
    return_if_matches!(MFSampleExtension_CaptureMetadata);
    return_if_matches!(MFSampleExtension_ChromaOnly);
    return_if_matches!(MFSampleExtension_CleanPoint);
    return_if_matches!(MFSampleExtension_ClosedCaption_CEA708);
    return_if_matches!(MFSampleExtension_Content_KeyID);
    return_if_matches!(MFSampleExtension_DecodeTimestamp);
    return_if_matches!(MFSampleExtension_Depth_MaxReliableDepth);
    return_if_matches!(MFSampleExtension_Depth_MinReliableDepth);
    return_if_matches!(MFSampleExtension_DerivedFromTopField);
    return_if_matches!(MFSampleExtension_DescrambleData);
    return_if_matches!(MFSampleExtension_DeviceReferenceSystemTime);
    return_if_matches!(MFSampleExtension_DeviceTimestamp);
    return_if_matches!(MFSampleExtension_DirtyRects);
    return_if_matches!(MFSampleExtension_Discontinuity);
    return_if_matches!(MFSampleExtension_Encryption_ClearSliceHeaderData);
    return_if_matches!(MFSampleExtension_Encryption_CryptByteBlock);
    return_if_matches!(MFSampleExtension_Encryption_HardwareProtection);
    return_if_matches!(MFSampleExtension_Encryption_HardwareProtection_KeyInfo);
    return_if_matches!(MFSampleExtension_Encryption_HardwareProtection_KeyInfoID);
    return_if_matches!(MFSampleExtension_Encryption_HardwareProtection_VideoDecryptorContext);
    return_if_matches!(MFSampleExtension_Encryption_KeyID);
    return_if_matches!(MFSampleExtension_Encryption_NALUTypes);
    return_if_matches!(MFSampleExtension_Encryption_Opaque_Data);
    return_if_matches!(MFSampleExtension_Encryption_ProtectionScheme);
    return_if_matches!(MFSampleExtension_Encryption_ResumeVideoOutput);
    return_if_matches!(MFSampleExtension_Encryption_SEIData);
    return_if_matches!(MFSampleExtension_Encryption_SPSPPSData);
    return_if_matches!(MFSampleExtension_Encryption_SampleID);
    return_if_matches!(MFSampleExtension_Encryption_SkipByteBlock);
    return_if_matches!(MFSampleExtension_Encryption_SubSampleMappingSplit);
    return_if_matches!(MFSampleExtension_Encryption_SubSample_Mapping);
    return_if_matches!(MFSampleExtension_ExtendedCameraIntrinsics);
    return_if_matches!(MFSampleExtension_FeatureMap);
    return_if_matches!(MFSampleExtension_ForwardedDecodeUnitType);
    return_if_matches!(MFSampleExtension_ForwardedDecodeUnits);
    return_if_matches!(MFSampleExtension_FrameCorruption);
    return_if_matches!(MFSampleExtension_GenKeyCtx);
    return_if_matches!(MFSampleExtension_GenKeyFunc);
    return_if_matches!(MFSampleExtension_HDCP_FrameCounter);
    return_if_matches!(MFSampleExtension_HDCP_OptionalHeader);
    return_if_matches!(MFSampleExtension_HDCP_StreamID);
    return_if_matches!(MFSampleExtension_Interlaced);
    return_if_matches!(MFSampleExtension_LastSlice);
    return_if_matches!(MFSampleExtension_LongTermReferenceFrameInfo);
    return_if_matches!(MFSampleExtension_MDLCacheCookie);
    return_if_matches!(MFSampleExtension_MULTIPLEXED_MANAGER);
    return_if_matches!(MFSampleExtension_MaxDecodeFrameSize);
    return_if_matches!(MFSampleExtension_MeanAbsoluteDifference);
    return_if_matches!(MFSampleExtension_MoveRegions);
    return_if_matches!(MFSampleExtension_NALULengthInfo);
    return_if_matches!(MFSampleExtension_PacketCrossOffsets);
    return_if_matches!(MFSampleExtension_PhotoThumbnail);
    return_if_matches!(MFSampleExtension_PhotoThumbnailMediaType);
    return_if_matches!(MFSampleExtension_PinholeCameraIntrinsics);
    return_if_matches!(MFSampleExtension_ROIRectangle);
    return_if_matches!(MFSampleExtension_RepeatFirstField);
    return_if_matches!(MFSampleExtension_RepeatFrame);
    return_if_matches!(MFSampleExtension_SampleKeyID);
    return_if_matches!(MFSampleExtension_SingleField);
    return_if_matches!(MFSampleExtension_Spatial_CameraCoordinateSystem);
    return_if_matches!(MFSampleExtension_Spatial_CameraProjectionTransform);
    return_if_matches!(MFSampleExtension_Spatial_CameraViewTransform);
    return_if_matches!(MFSampleExtension_TargetGlobalLuminance);
    return_if_matches!(MFSampleExtension_Timestamp);
    return_if_matches!(MFSampleExtension_Token);
    return_if_matches!(MFSampleExtension_VideoDSPMode);
    return_if_matches!(MFSampleExtension_VideoEncodePictureType);
    return_if_matches!(MFSampleExtension_VideoEncodeQP);
    return_if_matches!(MFStreamExtension_CameraExtrinsics);
    return_if_matches!(MFStreamExtension_ExtendedCameraIntrinsics);
    return_if_matches!(MFStreamExtension_PinholeCameraIntrinsics);
    return_if_matches!(MFStreamFormat_MPEG2Program);
    return_if_matches!(MFStreamFormat_MPEG2Transport);
    return_if_matches!(MFSubtitleFormat_ATSC);
    return_if_matches!(MFSubtitleFormat_CustomUserData);
    return_if_matches!(MFSubtitleFormat_PGS);
    return_if_matches!(MFSubtitleFormat_SRT);
    return_if_matches!(MFSubtitleFormat_SSA);
    return_if_matches!(MFSubtitleFormat_TTML);
    return_if_matches!(MFSubtitleFormat_VobSub);
    return_if_matches!(MFSubtitleFormat_WebVTT);
    return_if_matches!(MFSubtitleFormat_XML);
    return_if_matches!(MFT_AUDIO_DECODER_AUDIO_ENDPOINT_ID);
    return_if_matches!(MFT_AUDIO_DECODER_DEGRADATION_INFO_ATTRIBUTE);
    return_if_matches!(MFT_AUDIO_DECODER_SPATIAL_METADATA_CLIENT);
    return_if_matches!(MFT_CATEGORY_AUDIO_DECODER);
    return_if_matches!(MFT_CATEGORY_AUDIO_EFFECT);
    return_if_matches!(MFT_CATEGORY_AUDIO_ENCODER);
    return_if_matches!(MFT_CATEGORY_DEMULTIPLEXER);
    return_if_matches!(MFT_CATEGORY_ENCRYPTOR);
    return_if_matches!(MFT_CATEGORY_MULTIPLEXER);
    return_if_matches!(MFT_CATEGORY_OTHER);
    return_if_matches!(MFT_CATEGORY_VIDEO_DECODER);
    return_if_matches!(MFT_CATEGORY_VIDEO_EFFECT);
    return_if_matches!(MFT_CATEGORY_VIDEO_ENCODER);
    return_if_matches!(MFT_CATEGORY_VIDEO_PROCESSOR);
    return_if_matches!(MFT_CATEGORY_VIDEO_RENDERER_EFFECT);
    return_if_matches!(MFT_CODEC_MERIT_Attribute);
    return_if_matches!(MFT_CONNECTED_STREAM_ATTRIBUTE);
    return_if_matches!(MFT_CONNECTED_TO_HW_STREAM);
    return_if_matches!(MFT_DECODER_EXPOSE_OUTPUT_TYPES_IN_NATIVE_ORDER);
    return_if_matches!(MFT_DECODER_FINAL_VIDEO_RESOLUTION_HINT);
    return_if_matches!(MFT_DECODER_QUALITY_MANAGEMENT_CUSTOM_CONTROL);
    return_if_matches!(MFT_DECODER_QUALITY_MANAGEMENT_RECOVERY_WITHOUT_ARTIFACTS);
    return_if_matches!(MFT_ENCODER_ERROR);
    return_if_matches!(MFT_ENCODER_SUPPORTS_CONFIG_EVENT);
    return_if_matches!(MFT_END_STREAMING_AWARE);
    return_if_matches!(MFT_ENUM_ADAPTER_LUID);
    return_if_matches!(MFT_ENUM_HARDWARE_URL_Attribute);
    return_if_matches!(MFT_ENUM_HARDWARE_VENDOR_ID_Attribute);
    return_if_matches!(MFT_ENUM_TRANSCODE_ONLY_ATTRIBUTE);
    return_if_matches!(MFT_ENUM_VIDEO_RENDERER_EXTENSION_PROFILE);
    return_if_matches!(MFT_FIELDOFUSE_UNLOCK_Attribute);
    return_if_matches!(MFT_FRIENDLY_NAME_Attribute);
    return_if_matches!(MFT_GFX_DRIVER_VERSION_ID_Attribute);
    return_if_matches!(MFT_HW_TIMESTAMP_WITH_QPC_Attribute);
    return_if_matches!(MFT_INPUT_TYPES_Attributes);
    return_if_matches!(MFT_OUTPUT_TYPES_Attributes);
    return_if_matches!(MFT_POLICY_SET_AWARE);
    return_if_matches!(MFT_PREFERRED_ENCODER_PROFILE);
    return_if_matches!(MFT_PREFERRED_OUTPUTTYPE_Attribute);
    return_if_matches!(MFT_PROCESS_LOCAL_Attribute);
    return_if_matches!(MFT_REMUX_MARK_I_PICTURE_AS_CLEAN_POINT);
    return_if_matches!(MFT_SUPPORT_3DVIDEO);
    return_if_matches!(MFT_SUPPORT_DYNAMIC_FORMAT_CHANGE);
    return_if_matches!(MFT_TRANSFORM_CLSID_Attribute);
    return_if_matches!(MFT_USING_HARDWARE_DRM);
    return_if_matches!(MFTranscodeContainerType_3GP);
    return_if_matches!(MFTranscodeContainerType_AC3);
    return_if_matches!(MFTranscodeContainerType_ADTS);
    return_if_matches!(MFTranscodeContainerType_AMR);
    return_if_matches!(MFTranscodeContainerType_ASF);
    return_if_matches!(MFTranscodeContainerType_AVI);
    return_if_matches!(MFTranscodeContainerType_FLAC);
    return_if_matches!(MFTranscodeContainerType_FMPEG4);
    return_if_matches!(MFTranscodeContainerType_MP3);
    return_if_matches!(MFTranscodeContainerType_MPEG2);
    return_if_matches!(MFTranscodeContainerType_MPEG4);
    return_if_matches!(MFTranscodeContainerType_WAVE);
    return_if_matches!(MFVideoFormat_420O);
    return_if_matches!(MFVideoFormat_A16B16G16R16F);
    return_if_matches!(MFVideoFormat_A2R10G10B10);
    return_if_matches!(MFVideoFormat_AI44);
    return_if_matches!(MFVideoFormat_ARGB32);
    return_if_matches!(MFVideoFormat_AV1);
    return_if_matches!(MFVideoFormat_AYUV);
    return_if_matches!(MFVideoFormat_Base);
    return_if_matches!(MFVideoFormat_Base_HDCP);
    return_if_matches!(MFVideoFormat_D16);
    return_if_matches!(MFVideoFormat_DV25);
    return_if_matches!(MFVideoFormat_DV50);
    return_if_matches!(MFVideoFormat_DVH1);
    return_if_matches!(MFVideoFormat_DVHD);
    return_if_matches!(MFVideoFormat_DVSD);
    return_if_matches!(MFVideoFormat_DVSL);
    return_if_matches!(MFVideoFormat_H263);
    return_if_matches!(MFVideoFormat_H264);
    return_if_matches!(MFVideoFormat_H264_ES);
    return_if_matches!(MFVideoFormat_H264_HDCP);
    return_if_matches!(MFVideoFormat_H265);
    return_if_matches!(MFVideoFormat_HEVC);
    return_if_matches!(MFVideoFormat_HEVC_ES);
    return_if_matches!(MFVideoFormat_HEVC_HDCP);
    return_if_matches!(MFVideoFormat_I420);
    return_if_matches!(MFVideoFormat_IYUV);
    return_if_matches!(MFVideoFormat_L16);
    return_if_matches!(MFVideoFormat_L8);
    return_if_matches!(MFVideoFormat_M4S2);
    return_if_matches!(MFVideoFormat_MJPG);
    return_if_matches!(MFVideoFormat_MP43);
    return_if_matches!(MFVideoFormat_MP4S);
    return_if_matches!(MFVideoFormat_MP4V);
    return_if_matches!(MFVideoFormat_MPEG2);
    return_if_matches!(MFVideoFormat_MPG1);
    return_if_matches!(MFVideoFormat_MSS1);
    return_if_matches!(MFVideoFormat_MSS2);
    return_if_matches!(MFVideoFormat_NV11);
    return_if_matches!(MFVideoFormat_NV12);
    return_if_matches!(MFVideoFormat_NV21);
    return_if_matches!(MFVideoFormat_ORAW);
    return_if_matches!(MFVideoFormat_P010);
    return_if_matches!(MFVideoFormat_P016);
    return_if_matches!(MFVideoFormat_P210);
    return_if_matches!(MFVideoFormat_P216);
    return_if_matches!(MFVideoFormat_RGB24);
    return_if_matches!(MFVideoFormat_RGB32);
    return_if_matches!(MFVideoFormat_RGB555);
    return_if_matches!(MFVideoFormat_RGB565);
    return_if_matches!(MFVideoFormat_RGB8);
    return_if_matches!(MFVideoFormat_Theora);
    return_if_matches!(MFVideoFormat_UYVY);
    return_if_matches!(MFVideoFormat_VP10);
    return_if_matches!(MFVideoFormat_VP80);
    return_if_matches!(MFVideoFormat_VP90);
    return_if_matches!(MFVideoFormat_WMV1);
    return_if_matches!(MFVideoFormat_WMV2);
    return_if_matches!(MFVideoFormat_WMV3);
    return_if_matches!(MFVideoFormat_WVC1);
    return_if_matches!(MFVideoFormat_Y210);
    return_if_matches!(MFVideoFormat_Y216);
    return_if_matches!(MFVideoFormat_Y410);
    return_if_matches!(MFVideoFormat_Y416);
    return_if_matches!(MFVideoFormat_Y41P);
    return_if_matches!(MFVideoFormat_Y41T);
    return_if_matches!(MFVideoFormat_Y42T);
    return_if_matches!(MFVideoFormat_YUY2);
    return_if_matches!(MFVideoFormat_YV12);
    return_if_matches!(MFVideoFormat_YVU9);
    return_if_matches!(MFVideoFormat_YVYU);
    return_if_matches!(MFVideoFormat_v210);
    return_if_matches!(MFVideoFormat_v216);
    return_if_matches!(MFVideoFormat_v410);
    return_if_matches!(MF_ACCESS_CONTROLLED_MEDIASOURCE_SERVICE);
    return_if_matches!(MF_ACTIVATE_CUSTOM_VIDEO_MIXER_ACTIVATE);
    return_if_matches!(MF_ACTIVATE_CUSTOM_VIDEO_MIXER_CLSID);
    return_if_matches!(MF_ACTIVATE_CUSTOM_VIDEO_MIXER_FLAGS);
    return_if_matches!(MF_ACTIVATE_CUSTOM_VIDEO_PRESENTER_ACTIVATE);
    return_if_matches!(MF_ACTIVATE_CUSTOM_VIDEO_PRESENTER_CLSID);
    return_if_matches!(MF_ACTIVATE_CUSTOM_VIDEO_PRESENTER_FLAGS);
    return_if_matches!(MF_ACTIVATE_MFT_LOCKED);
    return_if_matches!(MF_ACTIVATE_VIDEO_WINDOW);
    return_if_matches!(MF_ASFPROFILE_MAXPACKETSIZE);
    return_if_matches!(MF_ASFPROFILE_MINPACKETSIZE);
    return_if_matches!(MF_ASFSTREAMCONFIG_LEAKYBUCKET1);
    return_if_matches!(MF_ASFSTREAMCONFIG_LEAKYBUCKET2);
    return_if_matches!(MF_AUDIO_RENDERER_ATTRIBUTE_ENDPOINT_ID);
    return_if_matches!(MF_AUDIO_RENDERER_ATTRIBUTE_ENDPOINT_ROLE);
    return_if_matches!(MF_AUDIO_RENDERER_ATTRIBUTE_FLAGS);
    return_if_matches!(MF_AUDIO_RENDERER_ATTRIBUTE_SESSION_ID);
    return_if_matches!(MF_AUDIO_RENDERER_ATTRIBUTE_STREAM_CATEGORY);
    return_if_matches!(MF_BD_MVC_PLANE_OFFSET_METADATA);
    return_if_matches!(MF_BYTESTREAMHANDLER_ACCEPTS_SHARE_WRITE);
    return_if_matches!(MF_BYTESTREAM_CONTENT_TYPE);
    return_if_matches!(MF_BYTESTREAM_DLNA_PROFILE_ID);
    return_if_matches!(MF_BYTESTREAM_DURATION);
    return_if_matches!(MF_BYTESTREAM_EFFECTIVE_URL);
    return_if_matches!(MF_BYTESTREAM_IFO_FILE_URI);
    return_if_matches!(MF_BYTESTREAM_LAST_MODIFIED_TIME);
    return_if_matches!(MF_BYTESTREAM_ORIGIN_NAME);
    return_if_matches!(MF_BYTESTREAM_SERVICE);
    return_if_matches!(MF_BYTESTREAM_TRANSCODED);
    return_if_matches!(MF_CAPTURE_ENGINE_ALL_EFFECTS_REMOVED);
    return_if_matches!(MF_CAPTURE_ENGINE_AUDIO_PROCESSING);
    return_if_matches!(MF_CAPTURE_ENGINE_CAMERA_STREAM_BLOCKED);
    return_if_matches!(MF_CAPTURE_ENGINE_CAMERA_STREAM_UNBLOCKED);
    return_if_matches!(MF_CAPTURE_ENGINE_D3D_MANAGER);
    return_if_matches!(MF_CAPTURE_ENGINE_DECODER_MFT_FIELDOFUSE_UNLOCK_Attribute);
    return_if_matches!(MF_CAPTURE_ENGINE_DISABLE_DXVA);
    return_if_matches!(MF_CAPTURE_ENGINE_DISABLE_HARDWARE_TRANSFORMS);
    return_if_matches!(MF_CAPTURE_ENGINE_EFFECT_ADDED);
    return_if_matches!(MF_CAPTURE_ENGINE_EFFECT_REMOVED);
    return_if_matches!(MF_CAPTURE_ENGINE_ENABLE_CAMERA_STREAMSTATE_NOTIFICATION);
    return_if_matches!(MF_CAPTURE_ENGINE_ENCODER_MFT_FIELDOFUSE_UNLOCK_Attribute);
    return_if_matches!(MF_CAPTURE_ENGINE_ERROR);
    return_if_matches!(MF_CAPTURE_ENGINE_EVENT_GENERATOR_GUID);
    return_if_matches!(MF_CAPTURE_ENGINE_EVENT_STREAM_INDEX);
    return_if_matches!(MF_CAPTURE_ENGINE_INITIALIZED);
    return_if_matches!(MF_CAPTURE_ENGINE_MEDIASOURCE_CONFIG);
    return_if_matches!(MF_CAPTURE_ENGINE_MEDIA_CATEGORY);
    return_if_matches!(MF_CAPTURE_ENGINE_OUTPUT_MEDIA_TYPE_SET);
    return_if_matches!(MF_CAPTURE_ENGINE_PHOTO_TAKEN);
    return_if_matches!(MF_CAPTURE_ENGINE_PREVIEW_STARTED);
    return_if_matches!(MF_CAPTURE_ENGINE_PREVIEW_STOPPED);
    return_if_matches!(MF_CAPTURE_ENGINE_RECORD_SINK_AUDIO_MAX_PROCESSED_SAMPLES);
    return_if_matches!(MF_CAPTURE_ENGINE_RECORD_SINK_AUDIO_MAX_UNPROCESSED_SAMPLES);
    return_if_matches!(MF_CAPTURE_ENGINE_RECORD_SINK_VIDEO_MAX_PROCESSED_SAMPLES);
    return_if_matches!(MF_CAPTURE_ENGINE_RECORD_SINK_VIDEO_MAX_UNPROCESSED_SAMPLES);
    return_if_matches!(MF_CAPTURE_ENGINE_RECORD_STARTED);
    return_if_matches!(MF_CAPTURE_ENGINE_RECORD_STOPPED);
    return_if_matches!(MF_CAPTURE_ENGINE_SELECTEDCAMERAPROFILE);
    return_if_matches!(MF_CAPTURE_ENGINE_SELECTEDCAMERAPROFILE_INDEX);
    return_if_matches!(MF_CAPTURE_ENGINE_USE_AUDIO_DEVICE_ONLY);
    return_if_matches!(MF_CAPTURE_ENGINE_USE_VIDEO_DEVICE_ONLY);
    return_if_matches!(MF_CAPTURE_METADATA_DIGITALWINDOW);
    return_if_matches!(MF_CAPTURE_METADATA_EXIF);
    return_if_matches!(MF_CAPTURE_METADATA_EXPOSURE_COMPENSATION);
    return_if_matches!(MF_CAPTURE_METADATA_EXPOSURE_TIME);
    return_if_matches!(MF_CAPTURE_METADATA_FACEROICHARACTERIZATIONS);
    return_if_matches!(MF_CAPTURE_METADATA_FACEROIS);
    return_if_matches!(MF_CAPTURE_METADATA_FACEROITIMESTAMPS);
    return_if_matches!(MF_CAPTURE_METADATA_FIRST_SCANLINE_START_TIME_QPC);
    return_if_matches!(MF_CAPTURE_METADATA_FLASH);
    return_if_matches!(MF_CAPTURE_METADATA_FLASH_POWER);
    return_if_matches!(MF_CAPTURE_METADATA_FOCUSSTATE);
    return_if_matches!(MF_CAPTURE_METADATA_FRAME_BACKGROUND_MASK);
    return_if_matches!(MF_CAPTURE_METADATA_FRAME_ILLUMINATION);
    return_if_matches!(MF_CAPTURE_METADATA_FRAME_RAWSTREAM);
    return_if_matches!(MF_CAPTURE_METADATA_HISTOGRAM);
    return_if_matches!(MF_CAPTURE_METADATA_ISO_GAINS);
    return_if_matches!(MF_CAPTURE_METADATA_ISO_SPEED);
    return_if_matches!(MF_CAPTURE_METADATA_LAST_SCANLINE_END_TIME_QPC);
    return_if_matches!(MF_CAPTURE_METADATA_LENS_POSITION);
    return_if_matches!(MF_CAPTURE_METADATA_PHOTO_FRAME_FLASH);
    return_if_matches!(MF_CAPTURE_METADATA_REQUESTED_FRAME_SETTING_ID);
    return_if_matches!(MF_CAPTURE_METADATA_SCANLINE_DIRECTION);
    return_if_matches!(MF_CAPTURE_METADATA_SCANLINE_TIME_QPC_ACCURACY);
    return_if_matches!(MF_CAPTURE_METADATA_SCENE_MODE);
    return_if_matches!(MF_CAPTURE_METADATA_SENSORFRAMERATE);
    return_if_matches!(MF_CAPTURE_METADATA_UVC_PAYLOADHEADER);
    return_if_matches!(MF_CAPTURE_METADATA_WHITEBALANCE);
    return_if_matches!(MF_CAPTURE_METADATA_WHITEBALANCE_GAINS);
    return_if_matches!(MF_CAPTURE_METADATA_ZOOMFACTOR);
    return_if_matches!(MF_CAPTURE_SINK_PREPARED);
    return_if_matches!(MF_CAPTURE_SOURCE_CURRENT_DEVICE_MEDIA_TYPE_SET);
    return_if_matches!(MF_CONTENTDECRYPTIONMODULE_SERVICE);
    return_if_matches!(MF_CONTENT_DECRYPTOR_SERVICE);
    return_if_matches!(MF_CONTENT_PROTECTION_DEVICE_SERVICE);
    return_if_matches!(MF_D3D12_SYNCHRONIZATION_OBJECT);
    return_if_matches!(MF_DECODER_FWD_CUSTOM_SEI_DECODE_ORDER);
    return_if_matches!(MF_DEVICEMFT_CONNECTED_FILTER_KSCONTROL);
    return_if_matches!(MF_DEVICEMFT_CONNECTED_PIN_KSCONTROL);
    return_if_matches!(MF_DEVICEMFT_EXTENSION_PLUGIN_CLSID);
    return_if_matches!(MF_DEVICEMFT_SENSORPROFILE_COLLECTION);
    return_if_matches!(MF_DEVICESTREAM_ATTRIBUTE_FACEAUTH_CAPABILITY);
    return_if_matches!(MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES);
    return_if_matches!(MF_DEVICESTREAM_ATTRIBUTE_SECURE_CAPABILITY);
    return_if_matches!(MF_DEVICESTREAM_EXTENSION_PLUGIN_CLSID);
    return_if_matches!(MF_DEVICESTREAM_EXTENSION_PLUGIN_CONNECTION_POINT);
    return_if_matches!(MF_DEVICESTREAM_FILTER_KSCONTROL);
    return_if_matches!(MF_DEVICESTREAM_FRAMESERVER_HIDDEN);
    return_if_matches!(MF_DEVICESTREAM_FRAMESERVER_SHARED);
    return_if_matches!(MF_DEVICESTREAM_IMAGE_STREAM);
    return_if_matches!(MF_DEVICESTREAM_INDEPENDENT_IMAGE_STREAM);
    return_if_matches!(MF_DEVICESTREAM_MAX_FRAME_BUFFERS);
    return_if_matches!(MF_DEVICESTREAM_MULTIPLEXED_MANAGER);
    return_if_matches!(MF_DEVICESTREAM_PIN_KSCONTROL);
    return_if_matches!(MF_DEVICESTREAM_REQUIRED_CAPABILITIES);
    return_if_matches!(MF_DEVICESTREAM_REQUIRED_SDDL);
    return_if_matches!(MF_DEVICESTREAM_SENSORSTREAM_ID);
    return_if_matches!(MF_DEVICESTREAM_SOURCE_ATTRIBUTES);
    return_if_matches!(MF_DEVICESTREAM_STREAM_CATEGORY);
    return_if_matches!(MF_DEVICESTREAM_STREAM_ID);
    return_if_matches!(MF_DEVICESTREAM_TAKEPHOTO_TRIGGER);
    return_if_matches!(MF_DEVICESTREAM_TRANSFORM_STREAM_ID);
    return_if_matches!(MF_DEVICE_THERMAL_STATE_CHANGED);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_ENABLE_MS_CAMERA_EFFECTS);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_MEDIA_TYPE);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_PASSWORD);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_STREAM_URL);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_AUDCAP_ENDPOINT_ID);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_AUDCAP_GUID);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_AUDCAP_ROLE);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_AUDCAP_SYMBOLIC_LINK);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_CATEGORY);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_HW_SOURCE);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_MAX_BUFFERS);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_PROVIDER_DEVICE_ID);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_USERNAME);
    return_if_matches!(MF_DEVSOURCE_ATTRIBUTE_SOURCE_XADDRESS);
    return_if_matches!(MF_DISABLE_FRAME_CORRUPTION_INFO);
    return_if_matches!(MF_DISABLE_LOCALLY_REGISTERED_PLUGINS);
    return_if_matches!(MF_DMFT_FRAME_BUFFER_INFO);
    return_if_matches!(MF_ENABLE_3DVIDEO_OUTPUT);
    return_if_matches!(MF_EVENT_DO_THINNING);
    return_if_matches!(MF_EVENT_MFT_CONTEXT);
    return_if_matches!(MF_EVENT_MFT_INPUT_STREAM_ID);
    return_if_matches!(MF_EVENT_OUTPUT_NODE);
    return_if_matches!(MF_EVENT_PRESENTATION_TIME_OFFSET);
    return_if_matches!(MF_EVENT_SCRUBSAMPLE_TIME);
    return_if_matches!(MF_EVENT_SESSIONCAPS);
    return_if_matches!(MF_EVENT_SESSIONCAPS_DELTA);
    return_if_matches!(MF_EVENT_SOURCE_ACTUAL_START);
    return_if_matches!(MF_EVENT_SOURCE_CHARACTERISTICS);
    return_if_matches!(MF_EVENT_SOURCE_CHARACTERISTICS_OLD);
    return_if_matches!(MF_EVENT_SOURCE_FAKE_START);
    return_if_matches!(MF_EVENT_SOURCE_PROJECTSTART);
    return_if_matches!(MF_EVENT_SOURCE_TOPOLOGY_CANCELED);
    return_if_matches!(MF_EVENT_START_PRESENTATION_TIME);
    return_if_matches!(MF_EVENT_START_PRESENTATION_TIME_AT_OUTPUT);
    return_if_matches!(MF_EVENT_STREAM_METADATA_CONTENT_KEYIDS);
    return_if_matches!(MF_EVENT_STREAM_METADATA_KEYDATA);
    return_if_matches!(MF_EVENT_STREAM_METADATA_SYSTEMID);
    return_if_matches!(MF_EVENT_TOPOLOGY_STATUS);
    return_if_matches!(MF_FRAMESERVER_VCAMEVENT_EXTENDED_CUSTOM_EVENT);
    return_if_matches!(MF_FRAMESERVER_VCAMEVENT_EXTENDED_PIPELINE_SHUTDOWN);
    return_if_matches!(MF_FRAMESERVER_VCAMEVENT_EXTENDED_SOURCE_INITIALIZE);
    return_if_matches!(MF_FRAMESERVER_VCAMEVENT_EXTENDED_SOURCE_START);
    return_if_matches!(MF_FRAMESERVER_VCAMEVENT_EXTENDED_SOURCE_STOP);
    return_if_matches!(MF_FRAMESERVER_VCAMEVENT_EXTENDED_SOURCE_UNINITIALIZE);
    return_if_matches!(MF_INDEPENDENT_STILL_IMAGE);
    return_if_matches!(MF_LOCAL_MFT_REGISTRATION_SERVICE);
    return_if_matches!(MF_LOCAL_PLUGIN_CONTROL_POLICY);
    return_if_matches!(MF_LOW_LATENCY);
    return_if_matches!(MF_LUMA_KEY_ENABLE);
    return_if_matches!(MF_LUMA_KEY_LOWER);
    return_if_matches!(MF_LUMA_KEY_UPPER);
    return_if_matches!(MF_MEDIASINK_AUTOFINALIZE_SUPPORTED);
    return_if_matches!(MF_MEDIASINK_ENABLE_AUTOFINALIZE);
    return_if_matches!(MF_MEDIASOURCE_EXPOSE_ALL_STREAMS);
    return_if_matches!(MF_MEDIASOURCE_SERVICE);
    return_if_matches!(MF_MEDIATYPE_MULTIPLEXED_MANAGER);
    return_if_matches!(MF_MEDIA_ENGINE_AUDIO_CATEGORY);
    return_if_matches!(MF_MEDIA_ENGINE_AUDIO_ENDPOINT_ROLE);
    return_if_matches!(MF_MEDIA_ENGINE_BROWSER_COMPATIBILITY_MODE);
    return_if_matches!(MF_MEDIA_ENGINE_BROWSER_COMPATIBILITY_MODE_IE10);
    return_if_matches!(MF_MEDIA_ENGINE_BROWSER_COMPATIBILITY_MODE_IE11);
    return_if_matches!(MF_MEDIA_ENGINE_BROWSER_COMPATIBILITY_MODE_IE9);
    return_if_matches!(MF_MEDIA_ENGINE_BROWSER_COMPATIBILITY_MODE_IE_EDGE);
    return_if_matches!(MF_MEDIA_ENGINE_CALLBACK);
    return_if_matches!(MF_MEDIA_ENGINE_COMPATIBILITY_MODE);
    return_if_matches!(MF_MEDIA_ENGINE_COMPATIBILITY_MODE_WIN10);
    return_if_matches!(MF_MEDIA_ENGINE_COMPATIBILITY_MODE_WWA_EDGE);
    return_if_matches!(MF_MEDIA_ENGINE_CONTENT_PROTECTION_FLAGS);
    return_if_matches!(MF_MEDIA_ENGINE_CONTENT_PROTECTION_MANAGER);
    return_if_matches!(MF_MEDIA_ENGINE_CONTINUE_ON_CODEC_ERROR);
    return_if_matches!(MF_MEDIA_ENGINE_COREWINDOW);
    return_if_matches!(MF_MEDIA_ENGINE_DXGI_MANAGER);
    return_if_matches!(MF_MEDIA_ENGINE_EME_CALLBACK);
    return_if_matches!(MF_MEDIA_ENGINE_EXTENSION);
    return_if_matches!(MF_MEDIA_ENGINE_MEDIA_PLAYER_MODE);
    return_if_matches!(MF_MEDIA_ENGINE_NEEDKEY_CALLBACK);
    return_if_matches!(MF_MEDIA_ENGINE_OPM_HWND);
    return_if_matches!(MF_MEDIA_ENGINE_PLAYBACK_HWND);
    return_if_matches!(MF_MEDIA_ENGINE_PLAYBACK_VISUAL);
    return_if_matches!(MF_MEDIA_ENGINE_SOURCE_RESOLVER_CONFIG_STORE);
    return_if_matches!(MF_MEDIA_ENGINE_STREAM_CONTAINS_ALPHA_CHANNEL);
    return_if_matches!(MF_MEDIA_ENGINE_SYNCHRONOUS_CLOSE);
    return_if_matches!(MF_MEDIA_ENGINE_TELEMETRY_APPLICATION_ID);
    return_if_matches!(MF_MEDIA_ENGINE_TIMEDTEXT);
    return_if_matches!(MF_MEDIA_ENGINE_TRACK_ID);
    return_if_matches!(MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT);
    return_if_matches!(MF_MEDIA_PROTECTION_MANAGER_PROPERTIES);
    return_if_matches!(MF_MEDIA_SHARING_ENGINE_DEVICE);
    return_if_matches!(MF_MEDIA_SHARING_ENGINE_DEVICE_NAME);
    return_if_matches!(MF_MEDIA_SHARING_ENGINE_INITIAL_SEEK_TIME);
    return_if_matches!(MF_METADATA_PROVIDER_SERVICE);
    return_if_matches!(MF_MP2DLNA_AUDIO_BIT_RATE);
    return_if_matches!(MF_MP2DLNA_ENCODE_QUALITY);
    return_if_matches!(MF_MP2DLNA_STATISTICS);
    return_if_matches!(MF_MP2DLNA_USE_MMCSS);
    return_if_matches!(MF_MP2DLNA_VIDEO_BIT_RATE);
    return_if_matches!(MF_MPEG4SINK_MAX_CODED_SEQUENCES_PER_FRAGMENT);
    return_if_matches!(MF_MPEG4SINK_MINIMUM_PROPERTIES_SIZE);
    return_if_matches!(MF_MPEG4SINK_MIN_FRAGMENT_DURATION);
    return_if_matches!(MF_MPEG4SINK_MOOV_BEFORE_MDAT);
    return_if_matches!(MF_MPEG4SINK_SPSPPS_PASSTHROUGH);
    return_if_matches!(MF_MSE_ACTIVELIST_CALLBACK);
    return_if_matches!(MF_MSE_BUFFERLIST_CALLBACK);
    return_if_matches!(MF_MSE_CALLBACK);
    return_if_matches!(MF_MSE_OPUS_SUPPORT);
    return_if_matches!(MF_MSE_VP9_SUPPORT);
    return_if_matches!(MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION);
    return_if_matches!(MF_MT_AAC_PAYLOAD_TYPE);
    return_if_matches!(MF_MT_ALL_SAMPLES_INDEPENDENT);
    return_if_matches!(MF_MT_ALPHA_MODE);
    return_if_matches!(MF_MT_AM_FORMAT_TYPE);
    return_if_matches!(MF_MT_ARBITRARY_FORMAT);
    return_if_matches!(MF_MT_ARBITRARY_HEADER);
    return_if_matches!(MF_MT_AUDIO_AVG_BYTES_PER_SECOND);
    return_if_matches!(MF_MT_AUDIO_BITS_PER_SAMPLE);
    return_if_matches!(MF_MT_AUDIO_BLOCK_ALIGNMENT);
    return_if_matches!(MF_MT_AUDIO_CHANNEL_MASK);
    return_if_matches!(MF_MT_AUDIO_FLAC_MAX_BLOCK_SIZE);
    return_if_matches!(MF_MT_AUDIO_FLOAT_SAMPLES_PER_SECOND);
    return_if_matches!(MF_MT_AUDIO_FOLDDOWN_MATRIX);
    return_if_matches!(MF_MT_AUDIO_NUM_CHANNELS);
    return_if_matches!(MF_MT_AUDIO_PREFER_WAVEFORMATEX);
    return_if_matches!(MF_MT_AUDIO_SAMPLES_PER_BLOCK);
    return_if_matches!(MF_MT_AUDIO_SAMPLES_PER_SECOND);
    return_if_matches!(MF_MT_AUDIO_VALID_BITS_PER_SAMPLE);
    return_if_matches!(MF_MT_AUDIO_WMADRC_AVGREF);
    return_if_matches!(MF_MT_AUDIO_WMADRC_AVGTARGET);
    return_if_matches!(MF_MT_AUDIO_WMADRC_PEAKREF);
    return_if_matches!(MF_MT_AUDIO_WMADRC_PEAKTARGET);
    return_if_matches!(MF_MT_AVG_BITRATE);
    return_if_matches!(MF_MT_AVG_BIT_ERROR_RATE);
    return_if_matches!(MF_MT_COMPRESSED);
    return_if_matches!(MF_MT_CONTAINER_RATE_SCALING);
    return_if_matches!(MF_MT_CUSTOM_VIDEO_PRIMARIES);
    return_if_matches!(MF_MT_D3D12_CPU_READBACK);
    return_if_matches!(MF_MT_D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER);
    return_if_matches!(MF_MT_D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL);
    return_if_matches!(MF_MT_D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET);
    return_if_matches!(MF_MT_D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS);
    return_if_matches!(MF_MT_D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    return_if_matches!(MF_MT_D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE);
    return_if_matches!(MF_MT_D3D12_TEXTURE_LAYOUT);
    return_if_matches!(MF_MT_D3D_RESOURCE_VERSION);
    return_if_matches!(MF_MT_DECODER_MAX_DPB_COUNT);
    return_if_matches!(MF_MT_DECODER_USE_MAX_RESOLUTION);
    return_if_matches!(MF_MT_DEFAULT_STRIDE);
    return_if_matches!(MF_MT_DEPTH_MEASUREMENT);
    return_if_matches!(MF_MT_DEPTH_VALUE_UNIT);
    return_if_matches!(MF_MT_DRM_FLAGS);
    return_if_matches!(MF_MT_DV_AAUX_CTRL_PACK_0);
    return_if_matches!(MF_MT_DV_AAUX_CTRL_PACK_1);
    return_if_matches!(MF_MT_DV_AAUX_SRC_PACK_0);
    return_if_matches!(MF_MT_DV_AAUX_SRC_PACK_1);
    return_if_matches!(MF_MT_DV_VAUX_CTRL_PACK);
    return_if_matches!(MF_MT_DV_VAUX_SRC_PACK);
    return_if_matches!(MF_MT_FIXED_SIZE_SAMPLES);
    return_if_matches!(MF_MT_FORWARD_CUSTOM_NALU);
    return_if_matches!(MF_MT_FORWARD_CUSTOM_SEI);
    return_if_matches!(MF_MT_FRAME_RATE);
    return_if_matches!(MF_MT_FRAME_RATE_RANGE_MAX);
    return_if_matches!(MF_MT_FRAME_RATE_RANGE_MIN);
    return_if_matches!(MF_MT_FRAME_SIZE);
    return_if_matches!(MF_MT_GEOMETRIC_APERTURE);
    return_if_matches!(MF_MT_H264_CAPABILITIES);
    return_if_matches!(MF_MT_H264_LAYOUT_PER_STREAM);
    return_if_matches!(MF_MT_H264_MAX_CODEC_CONFIG_DELAY);
    return_if_matches!(MF_MT_H264_MAX_MB_PER_SEC);
    return_if_matches!(MF_MT_H264_RATE_CONTROL_MODES);
    return_if_matches!(MF_MT_H264_RESOLUTION_SCALING);
    return_if_matches!(MF_MT_H264_SIMULCAST_SUPPORT);
    return_if_matches!(MF_MT_H264_SUPPORTED_RATE_CONTROL_MODES);
    return_if_matches!(MF_MT_H264_SUPPORTED_SLICE_MODES);
    return_if_matches!(MF_MT_H264_SUPPORTED_SYNC_FRAME_TYPES);
    return_if_matches!(MF_MT_H264_SUPPORTED_USAGES);
    return_if_matches!(MF_MT_H264_SVC_CAPABILITIES);
    return_if_matches!(MF_MT_H264_USAGE);
    return_if_matches!(MF_MT_IMAGE_LOSS_TOLERANT);
    return_if_matches!(MF_MT_INTERLACE_MODE);
    return_if_matches!(MF_MT_IN_BAND_PARAMETER_SET);
    return_if_matches!(MF_MT_MAJOR_TYPE);
    return_if_matches!(MF_MT_MAX_FRAME_AVERAGE_LUMINANCE_LEVEL);
    return_if_matches!(MF_MT_MAX_KEYFRAME_SPACING);
    return_if_matches!(MF_MT_MAX_LUMINANCE_LEVEL);
    return_if_matches!(MF_MT_MAX_MASTERING_LUMINANCE);
    return_if_matches!(MF_MT_MINIMUM_DISPLAY_APERTURE);
    return_if_matches!(MF_MT_MIN_MASTERING_LUMINANCE);
    return_if_matches!(MF_MT_MPEG2_CONTENT_PACKET);
    return_if_matches!(MF_MT_MPEG2_FLAGS);
    return_if_matches!(MF_MT_MPEG2_HDCP);
    return_if_matches!(MF_MT_MPEG2_LEVEL);
    return_if_matches!(MF_MT_MPEG2_ONE_FRAME_PER_PACKET);
    return_if_matches!(MF_MT_MPEG2_PROFILE);
    return_if_matches!(MF_MT_MPEG2_STANDARD);
    return_if_matches!(MF_MT_MPEG2_TIMECODE);
    return_if_matches!(MF_MT_MPEG4_CURRENT_SAMPLE_ENTRY);
    return_if_matches!(MF_MT_MPEG4_SAMPLE_DESCRIPTION);
    return_if_matches!(MF_MT_MPEG4_TRACK_TYPE);
    return_if_matches!(MF_MT_MPEG_SEQUENCE_HEADER);
    return_if_matches!(MF_MT_MPEG_START_TIME_CODE);
    return_if_matches!(MF_MT_ORIGINAL_4CC);
    return_if_matches!(MF_MT_ORIGINAL_WAVE_FORMAT_TAG);
    return_if_matches!(MF_MT_OUTPUT_BUFFER_NUM);
    return_if_matches!(MF_MT_PAD_CONTROL_FLAGS);
    return_if_matches!(MF_MT_PALETTE);
    return_if_matches!(MF_MT_PAN_SCAN_APERTURE);
    return_if_matches!(MF_MT_PAN_SCAN_ENABLED);
    return_if_matches!(MF_MT_PIXEL_ASPECT_RATIO);
    return_if_matches!(MF_MT_REALTIME_CONTENT);
    return_if_matches!(MF_MT_SAMPLE_SIZE);
    return_if_matches!(MF_MT_SECURE);
    return_if_matches!(MF_MT_SOURCE_CONTENT_HINT);
    return_if_matches!(MF_MT_SPATIAL_AUDIO_DATA_PRESENT);
    return_if_matches!(MF_MT_SPATIAL_AUDIO_MAX_DYNAMIC_OBJECTS);
    return_if_matches!(MF_MT_SPATIAL_AUDIO_MAX_METADATA_ITEMS);
    return_if_matches!(MF_MT_SPATIAL_AUDIO_MIN_METADATA_ITEM_OFFSET_SPACING);
    return_if_matches!(MF_MT_SPATIAL_AUDIO_OBJECT_METADATA_FORMAT_ID);
    return_if_matches!(MF_MT_SPATIAL_AUDIO_OBJECT_METADATA_LENGTH);
    return_if_matches!(MF_MT_SUBTYPE);
    return_if_matches!(MF_MT_TIMESTAMP_CAN_BE_DTS);
    return_if_matches!(MF_MT_TRANSFER_FUNCTION);
    return_if_matches!(MF_MT_USER_DATA);
    return_if_matches!(MF_MT_VIDEO_3D);
    return_if_matches!(MF_MT_VIDEO_3D_FIRST_IS_LEFT);
    return_if_matches!(MF_MT_VIDEO_3D_FORMAT);
    return_if_matches!(MF_MT_VIDEO_3D_LEFT_IS_BASE);
    return_if_matches!(MF_MT_VIDEO_3D_NUM_VIEWS);
    return_if_matches!(MF_MT_VIDEO_CHROMA_SITING);
    return_if_matches!(MF_MT_VIDEO_H264_NO_FMOASO);
    return_if_matches!(MF_MT_VIDEO_LEVEL);
    return_if_matches!(MF_MT_VIDEO_LIGHTING);
    return_if_matches!(MF_MT_VIDEO_NOMINAL_RANGE);
    return_if_matches!(MF_MT_VIDEO_NO_FRAME_ORDERING);
    return_if_matches!(MF_MT_VIDEO_PRIMARIES);
    return_if_matches!(MF_MT_VIDEO_PROFILE);
    return_if_matches!(MF_MT_VIDEO_RENDERER_EXTENSION_PROFILE);
    return_if_matches!(MF_MT_VIDEO_ROTATION);
    return_if_matches!(MF_MT_WRAPPED_TYPE);
    return_if_matches!(MF_MT_YUV_MATRIX);
    return_if_matches!(MF_NALU_LENGTH_INFORMATION);
    return_if_matches!(MF_NALU_LENGTH_SET);
    return_if_matches!(MF_PD_ADAPTIVE_STREAMING);
    return_if_matches!(MF_PD_APP_CONTEXT);
    return_if_matches!(MF_PD_ASF_CODECLIST);
    return_if_matches!(MF_PD_ASF_CONTENTENCRYPTIONEX_ENCRYPTION_DATA);
    return_if_matches!(MF_PD_ASF_CONTENTENCRYPTION_KEYID);
    return_if_matches!(MF_PD_ASF_CONTENTENCRYPTION_LICENSE_URL);
    return_if_matches!(MF_PD_ASF_CONTENTENCRYPTION_SECRET_DATA);
    return_if_matches!(MF_PD_ASF_CONTENTENCRYPTION_TYPE);
    return_if_matches!(MF_PD_ASF_DATA_LENGTH);
    return_if_matches!(MF_PD_ASF_DATA_START_OFFSET);
    return_if_matches!(MF_PD_ASF_FILEPROPERTIES_CREATION_TIME);
    return_if_matches!(MF_PD_ASF_FILEPROPERTIES_FILE_ID);
    return_if_matches!(MF_PD_ASF_FILEPROPERTIES_FLAGS);
    return_if_matches!(MF_PD_ASF_FILEPROPERTIES_MAX_BITRATE);
    return_if_matches!(MF_PD_ASF_FILEPROPERTIES_MAX_PACKET_SIZE);
    return_if_matches!(MF_PD_ASF_FILEPROPERTIES_MIN_PACKET_SIZE);
    return_if_matches!(MF_PD_ASF_FILEPROPERTIES_PACKETS);
    return_if_matches!(MF_PD_ASF_FILEPROPERTIES_PLAY_DURATION);
    return_if_matches!(MF_PD_ASF_FILEPROPERTIES_PREROLL);
    return_if_matches!(MF_PD_ASF_FILEPROPERTIES_SEND_DURATION);
    return_if_matches!(MF_PD_ASF_INFO_HAS_AUDIO);
    return_if_matches!(MF_PD_ASF_INFO_HAS_NON_AUDIO_VIDEO);
    return_if_matches!(MF_PD_ASF_INFO_HAS_VIDEO);
    return_if_matches!(MF_PD_ASF_LANGLIST);
    return_if_matches!(MF_PD_ASF_LANGLIST_LEGACYORDER);
    return_if_matches!(MF_PD_ASF_MARKER);
    return_if_matches!(MF_PD_ASF_METADATA_IS_VBR);
    return_if_matches!(MF_PD_ASF_METADATA_LEAKY_BUCKET_PAIRS);
    return_if_matches!(MF_PD_ASF_METADATA_V8_BUFFERAVERAGE);
    return_if_matches!(MF_PD_ASF_METADATA_V8_VBRPEAK);
    return_if_matches!(MF_PD_ASF_SCRIPT);
    return_if_matches!(MF_PD_AUDIO_ENCODING_BITRATE);
    return_if_matches!(MF_PD_AUDIO_ISVARIABLEBITRATE);
    return_if_matches!(MF_PD_DURATION);
    return_if_matches!(MF_PD_LAST_MODIFIED_TIME);
    return_if_matches!(MF_PD_MIME_TYPE);
    return_if_matches!(MF_PD_PLAYBACK_BOUNDARY_TIME);
    return_if_matches!(MF_PD_PLAYBACK_ELEMENT_ID);
    return_if_matches!(MF_PD_PMPHOST_CONTEXT);
    return_if_matches!(MF_PD_PREFERRED_LANGUAGE);
    return_if_matches!(MF_PD_SAMI_STYLELIST);
    return_if_matches!(MF_PD_TOTAL_FILE_SIZE);
    return_if_matches!(MF_PD_VIDEO_ENCODING_BITRATE);
    return_if_matches!(MF_PMP_SERVER_CONTEXT);
    return_if_matches!(MF_POLICY_ID);
    return_if_matches!(MF_PREFERRED_SOURCE_URI);
    return_if_matches!(MF_PROGRESSIVE_CODING_CONTENT);
    return_if_matches!(MF_PROPERTY_HANDLER_SERVICE);
    return_if_matches!(MF_QUALITY_NOTIFY_PROCESSING_LATENCY);
    return_if_matches!(MF_QUALITY_NOTIFY_SAMPLE_LAG);
    return_if_matches!(MF_QUALITY_SERVICES);
    return_if_matches!(MF_RATE_CONTROL_SERVICE);
    return_if_matches!(MF_READWRITE_D3D_OPTIONAL);
    return_if_matches!(MF_READWRITE_DISABLE_CONVERTERS);
    return_if_matches!(MF_READWRITE_ENABLE_AUTOFINALIZE);
    return_if_matches!(MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS);
    return_if_matches!(MF_READWRITE_MMCSS_CLASS);
    return_if_matches!(MF_READWRITE_MMCSS_CLASS_AUDIO);
    return_if_matches!(MF_READWRITE_MMCSS_PRIORITY);
    return_if_matches!(MF_READWRITE_MMCSS_PRIORITY_AUDIO);
    return_if_matches!(MF_REMOTE_PROXY);
    return_if_matches!(MF_SAMI_SERVICE);
    return_if_matches!(MF_SAMPLEGRABBERSINK_IGNORE_CLOCK);
    return_if_matches!(MF_SAMPLEGRABBERSINK_SAMPLE_TIME_OFFSET);
    return_if_matches!(MF_SA_AUDIO_ENDPOINT_AWARE);
    return_if_matches!(MF_SA_BUFFERS_PER_SAMPLE);
    return_if_matches!(MF_SA_D3D11_ALLOCATE_DISPLAYABLE_RESOURCES);
    return_if_matches!(MF_SA_D3D11_ALLOW_DYNAMIC_YUV_TEXTURE);
    return_if_matches!(MF_SA_D3D11_AWARE);
    return_if_matches!(MF_SA_D3D11_BINDFLAGS);
    return_if_matches!(MF_SA_D3D11_HW_PROTECTED);
    return_if_matches!(MF_SA_D3D11_SHARED);
    return_if_matches!(MF_SA_D3D11_SHARED_WITHOUT_MUTEX);
    return_if_matches!(MF_SA_D3D11_USAGE);
    return_if_matches!(MF_SA_D3D12_CLEAR_VALUE);
    return_if_matches!(MF_SA_D3D12_HEAP_FLAGS);
    return_if_matches!(MF_SA_D3D12_HEAP_TYPE);
    return_if_matches!(MF_SA_D3D_AWARE);
    return_if_matches!(MF_SA_MINIMUM_OUTPUT_SAMPLE_COUNT);
    return_if_matches!(MF_SA_MINIMUM_OUTPUT_SAMPLE_COUNT_PROGRESSIVE);
    return_if_matches!(MF_SA_REQUIRED_SAMPLE_COUNT);
    return_if_matches!(MF_SA_REQUIRED_SAMPLE_COUNT_PROGRESSIVE);
    return_if_matches!(MF_SD_AMBISONICS_SAMPLE3D_DESCRIPTION);
    return_if_matches!(MF_SD_ASF_EXTSTRMPROP_AVG_BUFFERSIZE);
    return_if_matches!(MF_SD_ASF_EXTSTRMPROP_AVG_DATA_BITRATE);
    return_if_matches!(MF_SD_ASF_EXTSTRMPROP_LANGUAGE_ID_INDEX);
    return_if_matches!(MF_SD_ASF_EXTSTRMPROP_MAX_BUFFERSIZE);
    return_if_matches!(MF_SD_ASF_EXTSTRMPROP_MAX_DATA_BITRATE);
    return_if_matches!(MF_SD_ASF_METADATA_DEVICE_CONFORMANCE_TEMPLATE);
    return_if_matches!(MF_SD_ASF_STREAMBITRATES_BITRATE);
    return_if_matches!(MF_SD_AUDIO_ENCODER_DELAY);
    return_if_matches!(MF_SD_AUDIO_ENCODER_PADDING);
    return_if_matches!(MF_SD_LANGUAGE);
    return_if_matches!(MF_SD_MEDIASOURCE_STATUS);
    return_if_matches!(MF_SD_MUTUALLY_EXCLUSIVE);
    return_if_matches!(MF_SD_PROTECTED);
    return_if_matches!(MF_SD_SAMI_LANGUAGE);
    return_if_matches!(MF_SD_STREAM_NAME);
    return_if_matches!(MF_SD_VIDEO_SPHERICAL);
    return_if_matches!(MF_SD_VIDEO_SPHERICAL_FORMAT);
    return_if_matches!(MF_SD_VIDEO_SPHERICAL_INITIAL_VIEWDIRECTION);
    return_if_matches!(MF_SESSION_APPROX_EVENT_OCCURRENCE_TIME);
    return_if_matches!(MF_SESSION_CONTENT_PROTECTION_MANAGER);
    return_if_matches!(MF_SESSION_GLOBAL_TIME);
    return_if_matches!(MF_SESSION_QUALITY_MANAGER);
    return_if_matches!(MF_SESSION_REMOTE_SOURCE_MODE);
    return_if_matches!(MF_SESSION_SERVER_CONTEXT);
    return_if_matches!(MF_SESSION_TOPOLOADER);
    return_if_matches!(MF_SHARING_ENGINE_CALLBACK);
    return_if_matches!(MF_SHARING_ENGINE_SHAREDRENDERER);
    return_if_matches!(MF_SHUTDOWN_RENDERER_ON_ENGINE_SHUTDOWN);
    return_if_matches!(MF_SINK_VIDEO_DISPLAY_ASPECT_RATIO_DENOMINATOR);
    return_if_matches!(MF_SINK_VIDEO_DISPLAY_ASPECT_RATIO_NUMERATOR);
    return_if_matches!(MF_SINK_VIDEO_NATIVE_HEIGHT);
    return_if_matches!(MF_SINK_VIDEO_NATIVE_WIDTH);
    return_if_matches!(MF_SINK_VIDEO_PTS);
    return_if_matches!(MF_SINK_WRITER_ASYNC_CALLBACK);
    return_if_matches!(MF_SINK_WRITER_D3D_MANAGER);
    return_if_matches!(MF_SINK_WRITER_DISABLE_THROTTLING);
    return_if_matches!(MF_SINK_WRITER_ENCODER_CONFIG);
    return_if_matches!(MF_SOURCE_PRESENTATION_PROVIDER_SERVICE);
    return_if_matches!(MF_SOURCE_READER_ASYNC_CALLBACK);
    return_if_matches!(MF_SOURCE_READER_D3D11_BIND_FLAGS);
    return_if_matches!(MF_SOURCE_READER_D3D_MANAGER);
    return_if_matches!(MF_SOURCE_READER_DISABLE_CAMERA_PLUGINS);
    return_if_matches!(MF_SOURCE_READER_DISABLE_DXVA);
    return_if_matches!(MF_SOURCE_READER_DISCONNECT_MEDIASOURCE_ON_SHUTDOWN);
    return_if_matches!(MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING);
    return_if_matches!(MF_SOURCE_READER_ENABLE_TRANSCODE_ONLY_TRANSFORMS);
    return_if_matches!(MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING);
    return_if_matches!(MF_SOURCE_READER_MEDIASOURCE_CHARACTERISTICS);
    return_if_matches!(MF_SOURCE_READER_MEDIASOURCE_CONFIG);
    return_if_matches!(MF_SOURCE_STREAM_SUPPORTS_HW_CONNECTION);
    return_if_matches!(MF_STF_VERSION_DATE);
    return_if_matches!(MF_STF_VERSION_INFO);
    return_if_matches!(MF_STREAM_SINK_SUPPORTS_HW_CONNECTION);
    return_if_matches!(MF_STREAM_SINK_SUPPORTS_ROTATION);
    return_if_matches!(MF_ST_MEDIASOURCE_COLLECTION);
    return_if_matches!(MF_SampleProtectionSalt);
    return_if_matches!(MF_TIMECODE_SERVICE);
    return_if_matches!(MF_TIME_FORMAT_ENTRY_RELATIVE);
    return_if_matches!(MF_TIME_FORMAT_SEGMENT_OFFSET);
    return_if_matches!(MF_TOPOLOGY_DXVA_MODE);
    return_if_matches!(MF_TOPOLOGY_DYNAMIC_CHANGE_NOT_ALLOWED);
    return_if_matches!(MF_TOPOLOGY_ENABLE_XVP_FOR_PLAYBACK);
    return_if_matches!(MF_TOPOLOGY_ENUMERATE_SOURCE_TYPES);
    return_if_matches!(MF_TOPOLOGY_HARDWARE_MODE);
    return_if_matches!(MF_TOPOLOGY_NO_MARKIN_MARKOUT);
    return_if_matches!(MF_TOPOLOGY_PLAYBACK_FRAMERATE);
    return_if_matches!(MF_TOPOLOGY_PLAYBACK_MAX_DIMS);
    return_if_matches!(MF_TOPOLOGY_PROJECTSTART);
    return_if_matches!(MF_TOPOLOGY_PROJECTSTOP);
    return_if_matches!(MF_TOPOLOGY_RESOLUTION_STATUS);
    return_if_matches!(MF_TOPOLOGY_START_TIME_ON_PRESENTATION_SWITCH);
    return_if_matches!(MF_TOPOLOGY_STATIC_PLAYBACK_OPTIMIZATIONS);
    return_if_matches!(MF_TOPONODE_ATTRIBUTE_EDITOR_SERVICE);
    return_if_matches!(MF_TOPONODE_CONNECT_METHOD);
    return_if_matches!(MF_TOPONODE_D3DAWARE);
    return_if_matches!(MF_TOPONODE_DECODER);
    return_if_matches!(MF_TOPONODE_DECRYPTOR);
    return_if_matches!(MF_TOPONODE_DISABLE_PREROLL);
    return_if_matches!(MF_TOPONODE_DISCARDABLE);
    return_if_matches!(MF_TOPONODE_DRAIN);
    return_if_matches!(MF_TOPONODE_ERRORCODE);
    return_if_matches!(MF_TOPONODE_ERROR_MAJORTYPE);
    return_if_matches!(MF_TOPONODE_ERROR_SUBTYPE);
    return_if_matches!(MF_TOPONODE_FLUSH);
    return_if_matches!(MF_TOPONODE_LOCKED);
    return_if_matches!(MF_TOPONODE_MARKIN_HERE);
    return_if_matches!(MF_TOPONODE_MARKOUT_HERE);
    return_if_matches!(MF_TOPONODE_MEDIASTART);
    return_if_matches!(MF_TOPONODE_MEDIASTOP);
    return_if_matches!(MF_TOPONODE_NOSHUTDOWN_ON_REMOVE);
    return_if_matches!(MF_TOPONODE_PRESENTATION_DESCRIPTOR);
    return_if_matches!(MF_TOPONODE_PRIMARYOUTPUT);
    return_if_matches!(MF_TOPONODE_RATELESS);
    return_if_matches!(MF_TOPONODE_SEQUENCE_ELEMENTID);
    return_if_matches!(MF_TOPONODE_SOURCE);
    return_if_matches!(MF_TOPONODE_STREAMID);
    return_if_matches!(MF_TOPONODE_STREAM_DESCRIPTOR);
    return_if_matches!(MF_TOPONODE_TRANSFORM_OBJECTID);
    return_if_matches!(MF_TOPONODE_WORKQUEUE_ID);
    return_if_matches!(MF_TOPONODE_WORKQUEUE_ITEM_PRIORITY);
    return_if_matches!(MF_TOPONODE_WORKQUEUE_MMCSS_CLASS);
    return_if_matches!(MF_TOPONODE_WORKQUEUE_MMCSS_PRIORITY);
    return_if_matches!(MF_TOPONODE_WORKQUEUE_MMCSS_TASKID);
    return_if_matches!(MF_TRANSCODE_ADJUST_PROFILE);
    return_if_matches!(MF_TRANSCODE_CONTAINERTYPE);
    return_if_matches!(MF_TRANSCODE_DONOT_INSERT_ENCODER);
    return_if_matches!(MF_TRANSCODE_ENCODINGPROFILE);
    return_if_matches!(MF_TRANSCODE_QUALITYVSSPEED);
    return_if_matches!(MF_TRANSCODE_SKIP_METADATA_TRANSFER);
    return_if_matches!(MF_TRANSCODE_TOPOLOGYMODE);
    return_if_matches!(MF_TRANSFORM_ASYNC);
    return_if_matches!(MF_TRANSFORM_ASYNC_UNLOCK);
    return_if_matches!(MF_TRANSFORM_CATEGORY_Attribute);
    return_if_matches!(MF_TRANSFORM_FLAGS_Attribute);
    return_if_matches!(MF_USER_DATA_PAYLOAD);
    return_if_matches!(MF_USER_EXTENDED_ATTRIBUTES);
    return_if_matches!(MF_VIDEODSP_MODE);
    return_if_matches!(MF_VIDEO_MAX_MB_PER_SEC);
    return_if_matches!(MF_VIDEO_PROCESSOR_ALGORITHM);
    return_if_matches!(MF_VIDEO_RENDERER_EFFECT_APP_SERVICE_NAME);
    return_if_matches!(MF_VIRTUALCAMERA_ASSOCIATED_CAMERA_SOURCES);
    return_if_matches!(MF_VIRTUALCAMERA_CONFIGURATION_APP_PACKAGE_FAMILY_NAME);
    return_if_matches!(MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES);
    return_if_matches!(MF_WORKQUEUE_SERVICES);
    return_if_matches!(MF_WRAPPED_BUFFER_SERVICE);
    return_if_matches!(MF_WRAPPED_OBJECT);
    return_if_matches!(MF_WRAPPED_SAMPLE_SERVICE);
    return_if_matches!(MF_WVC1_PROG_SINGLE_SLICE_CONTENT);
    return_if_matches!(MF_XVP_CALLER_ALLOCATES_OUTPUT);
    return_if_matches!(MF_XVP_DISABLE_FRC);
    return_if_matches!(MF_XVP_SAMPLE_LOCK_TIMEOUT);
    return_if_matches!(MFAMRNBByteStreamHandler);
    return_if_matches!(MFAMRNBSinkClassFactory);
    return_if_matches!(MFFLACBytestreamHandler);
    return_if_matches!(MFFLACSinkClassFactory);

    return format!("{guid:?}").into();
}

#[derive(std::fmt::Debug)]
pub(crate) enum VariantType {
    VtEmpty = 0x0000,
    VtNull = 0x0001,
    VtI2 = 0x0002,
    VtI4 = 0x0003,
    VtR4 = 0x0004,
    VtR8 = 0x0005,
    VtCy = 0x0006,
    VtDate = 0x0007,
    VtBstr = 0x0008,
    VtDispatch = 0x0009,
    VtError = 0x000A,
    VtBool = 0x000B,
    VtVariant = 0x000C,
    VtUnknown = 0x000D,
    VtDecimal = 0x000E,
    VtI1 = 0x0010,
    VtUi1 = 0x0011,
    VtUi2 = 0x0012,
    VtUi4 = 0x0013,
    VtI8 = 0x0014,
    VtUi8 = 0x0015,
    VtInt = 0x0016,
    VtUint = 0x0017,
    VtVoid = 0x0018,
    VtHresult = 0x0019,
    VtPtr = 0x001A,
    VtSafearray = 0x001B,
    VtCarray = 0x001C,
    VtUserdefined = 0x001D,
    VtLpstr = 0x001E,
    VtLpwstr = 0x001F,
    VtRecord = 0x0024,
    VtIntPtr = 0x0025,
    VtUintPtr = 0x0026,
    VtArray = 0x2000,
    VtByref = 0x4000,
}

#[derive(Debug)]
pub struct TryFromViariantTypeError;

impl std::fmt::Display for TryFromViariantTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TryFromVariantTypeError")
    }
}

impl std::error::Error for TryFromViariantTypeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }

    fn description(&self) -> &str {
        "description() is deprecated; use Display"
    }

    fn cause(&self) -> Option<&dyn std::error::Error> {
        self.source()
    }
}

impl TryFrom<u16> for VariantType {
    type Error = TryFromViariantTypeError;

    fn try_from(value: u16) -> std::prelude::v1::Result<Self, Self::Error> {
        match value {
            0x0000 => Ok(VariantType::VtEmpty),
            0x0001 => Ok(VariantType::VtNull),
            0x0002 => Ok(VariantType::VtI2),
            0x0003 => Ok(VariantType::VtI4),
            0x0004 => Ok(VariantType::VtR4),
            0x0005 => Ok(VariantType::VtR8),
            0x0006 => Ok(VariantType::VtCy),
            0x0007 => Ok(VariantType::VtDate),
            0x0008 => Ok(VariantType::VtBstr),
            0x0009 => Ok(VariantType::VtDispatch),
            0x000A => Ok(VariantType::VtError),
            0x000B => Ok(VariantType::VtBool),
            0x000C => Ok(VariantType::VtVariant),
            0x000D => Ok(VariantType::VtUnknown),
            0x000E => Ok(VariantType::VtDecimal),
            0x0010 => Ok(VariantType::VtI1),
            0x0011 => Ok(VariantType::VtUi1),
            0x0012 => Ok(VariantType::VtUi2),
            0x0013 => Ok(VariantType::VtUi4),
            0x0014 => Ok(VariantType::VtI8),
            0x0015 => Ok(VariantType::VtUi8),
            0x0016 => Ok(VariantType::VtInt),
            0x0017 => Ok(VariantType::VtUint),
            0x0018 => Ok(VariantType::VtVoid),
            0x0019 => Ok(VariantType::VtHresult),
            0x001A => Ok(VariantType::VtPtr),
            0x001B => Ok(VariantType::VtSafearray),
            0x001C => Ok(VariantType::VtCarray),
            0x001D => Ok(VariantType::VtUserdefined),
            0x001E => Ok(VariantType::VtLpstr),
            0x001F => Ok(VariantType::VtLpwstr),
            0x0024 => Ok(VariantType::VtRecord),
            0x0025 => Ok(VariantType::VtIntPtr),
            0x0026 => Ok(VariantType::VtUintPtr),
            0x2000 => Ok(VariantType::VtArray),
            0x4000 => Ok(VariantType::VtByref),
            _ => Err(TryFromViariantTypeError),
        }
    }
}

#[derive(Debug)]
pub struct TryFromPropViariantTypeError;

impl std::fmt::Display for TryFromPropViariantTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TryFromVariantTypeError")
    }
}

impl std::error::Error for TryFromPropViariantTypeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }

    fn description(&self) -> &str {
        "description() is deprecated; use Display"
    }

    fn cause(&self) -> Option<&dyn std::error::Error> {
        self.source()
    }
}

#[derive(Debug)]
pub enum PropVariantType {
    VtEmpty = 0,
    VtNull = 1,
    VtI2 = 2,
    VtI4 = 3,
    VtBool = 11,
    VtVariant = 12,
    VtI1 = 16,
    VtUi1 = 17,
    VtUi2 = 18,
    VtUi4 = 19,
    VtI8 = 20,
    VtUi8 = 21,
    VtLpwstr = 31,
    VtBlob = 65,
    VtClsid = 72,
}

impl TryFrom<u16> for PropVariantType {
    type Error = TryFromPropViariantTypeError;

    fn try_from(value: u16) -> std::prelude::v1::Result<Self, Self::Error> {
        match value {
            0 => Ok(PropVariantType::VtEmpty),
            1 => Ok(PropVariantType::VtNull),
            2 => Ok(PropVariantType::VtI2),
            3 => Ok(PropVariantType::VtI4),
            11 => Ok(PropVariantType::VtBool),
            12 => Ok(PropVariantType::VtVariant),
            16 => Ok(PropVariantType::VtI1),
            17 => Ok(PropVariantType::VtUi1),
            18 => Ok(PropVariantType::VtUi2),
            19 => Ok(PropVariantType::VtUi4),
            20 => Ok(PropVariantType::VtI8),
            21 => Ok(PropVariantType::VtUi8),
            31 => Ok(PropVariantType::VtLpwstr),
            65 => Ok(PropVariantType::VtBlob),
            72 => Ok(PropVariantType::VtClsid),
            _ => Err(TryFromPropViariantTypeError),
        }
    }
}
