use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

use anyhow::Result;
use pipewire as pw;
use pipewire::context::Context;
use pipewire::core::Core;
use pipewire::keys;
use pipewire::loop_::Loop;
use pipewire::properties::properties;
use pipewire::spa;
use pipewire::spa::param::format::MediaSubtype;
use pipewire::spa::param::format::MediaType;
use pipewire::spa::param::video::VideoInfoRaw;
use pipewire::spa::pod::Pod;
use pipewire::spa::pod::Value;
use pipewire::stream::{Stream, StreamFlags, StreamListener};
use pipewire::properties;

use crate::traits::Capture;
use crate::types::{CapturedFrame, CaptureConfig, OutputInfo};
use crate::types::{CpuBuffer, PixelFormat, VideoFrame};

use super::portal::{start_screen_capture_session, PortalSession};

pub struct LinuxCapture {
    portal_session: Option<PortalSession>,
    pipewire_loop: Option<Loop>,
    current_frame: Option<VideoFrame>,
    width: u32,
    height: u32,
}

impl LinuxCapture {
    pub fn new() -> Self {
        Self {
            portal_session: None,
            pipewire_loop: None,
            current_frame: None,
            width: 0,
            height: 0,
        }
    }
}

impl Capture for LinuxCapture {
    fn enumerate_outputs(&self) -> Result<Vec<OutputInfo>> {
        Ok(vec![OutputInfo {
            id: 0,
            name: "Screen".to_string(),
            width: 1920,
            height: 1080,
            is_primary: true,
        }])
    }

    fn start(&mut self, config: CaptureConfig) -> Result<()> {
        let rt = tokio::runtime::Handle::current();
        
        let portal_session = rt.block_on(async {
            start_screen_capture_session(config.capture_cursor).await
        })?;
        
        let stream = portal_session.streams.first()
            .ok_or_else(|| anyhow::anyhow!("No streams available"))?;
        
        self.width = stream.size.0;
        self.height = stream.size.1;
        
        let loop_ = Loop::new(None)?;
        
        let context = Context::new(&loop_)?;
        let core = context.connect(None)?;
        
        let props = properties! {
            *keys::MEDIA_TYPE => "Video",
            *keys::MEDIA_CATEGORY => "Capture",
            *keys::MEDIA_ROLE => "Screen",
            *keys::NODE_TARGET => stream.id.to_string(),
        };
        
        let pipewire_fd = portal_session.pipewire_fd.as_raw_fd();
        
        let stream = Stream::new_with_fd(
            &core,
            "remote-desktop-capture",
            props,
            pipewire_fd,
        )?;
        
        let width = self.width;
        let height = self.height;
        let current_frame = &mut self.current_frame;
        
        let listener = stream.add_listener()
            .state_changed(|_, old, new| {
                tracing::debug!("Stream state changed: {:?} -> {:?}", old, new);
            })
            .param_changed(move |_, id, param| {
                if id == spa::param::ParamType::Format.as_raw() {
                    if let Some(p) = param {
                        let media_type = p.parse::<MediaType>().ok();
                        let media_subtype = p.parse::<MediaSubtype>().ok();
                        
                        if media_type == Some(MediaType::Video) && media_subtype == Some(MediaSubtype::Raw) {
                            let info = p.parse::<VideoInfoRaw>().ok();
                            if let Some(info) = info {
                                tracing::debug!("Video format: {:?} {}x{}", 
                                    info.format(), info.size().width, info.size().height);
                            }
                        }
                    }
                }
            })
            .process(move |_, _| {
                tracing::trace!("Process callback");
            })
            .register()?;
        
        let obj = pw::spa::pod::object!(
            spa::utils::SpaTypes::ObjectParamFormat,
            spa::param::ParamType::Format,
            spa::pod::property!(
                spa::format::FormatProperties::MediaType,
                spa::pod::Value::Enum(MediaType::Video)
            ),
            spa::pod::property!(
                spa::format::FormatProperties::MediaSubtype,
                spa::pod::Value::Enum(MediaSubtype::Raw)
            ),
            spa::pod::property!(
                spa::format::FormatProperties::VideoFormat,
                spa::pod::Value::Enum(spa::format::VideoFormat::Bgra)
            ),
            spa::pod::property!(
                spa::format::FormatProperties::VideoSize,
                spa::pod::Value::Rectangle(spa::utils::Rectangle { width: width as i32, height: height as i32 })
            ),
            spa::pod::property!(
                spa::format::FormatProperties::VideoFramerate,
                spa::pod::Value::Fraction(spa::utils::Fraction { num: 60, denom: 1 })
            )
        );
        
        let pod = Pod::from(&obj);
        let params = &[pod.as_raw()];
        
        stream.connect(
            spa::param::ParamType::EnumFormat,
            Some(&pw::spa::pod::Pod::from(&obj)),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        )?;
        
        self.pipewire_loop = Some(loop_);
        self.portal_session = Some(portal_session);
        
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.pipewire_loop = None;
        self.portal_session = None;
        self.current_frame = None;
        Ok(())
    }

    fn next_frame(&mut self, _timeout: Duration) -> Result<Option<CapturedFrame>> {
        if let Some(ref loop_) = self.pipewire_loop {
            loop_.iterate(0);
        }
        
        if let Some(frame) = self.current_frame.take() {
            Ok(Some(CapturedFrame {
                frame,
                timestamp: Duration::from_secs(0),
                duration: Duration::from_secs_f64(1.0 / 60.0),
            }))
        } else {
            Ok(None)
        }
    }
}

impl Default for LinuxCapture {
    fn default() -> Self {
        Self::new()
    }
}
