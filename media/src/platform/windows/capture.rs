use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use windows::{
    core::Interface,
    Win32::{
        Foundation::E_ACCESSDENIED,
        Graphics::{
            Direct3D11::{
                ID3D11Texture2D, D3D11_BOX, D3D11_RESOURCE_MISC_FLAG,
                D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_TEXTURE2D_DESC,
            },
            Dxgi::{
                IDXGIAdapter, IDXGIDevice2, IDXGIKeyedMutex, IDXGIOutput1, IDXGIResource,
                DXGI_ERROR_ACCESS_DENIED, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_INVALID_CALL,
                DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTPUT_DESC,
            },
        },
    },
};

use crate::dx::{self, ID3D11Texture2DExt};
use crate::texture_pool::{Texture, TexturePool};
use crate::traits::Capture;
use crate::types::{CaptureConfig, CapturedFrame, OutputInfo, Timestamp, VideoFrame};
use crate::ARBITRARY_MEDIA_CHANNEL_LIMIT;

pub struct WindowsCapture {
    control_tx: Option<mpsc::Sender<CaptureControl>>,
    event_rx: Option<mpsc::Receiver<CaptureEvent>>,
    width: u32,
    height: u32,
}

enum CaptureControl {
    Stop,
}

enum CaptureEvent {
    Frame(Texture, Timestamp),
    Error(anyhow::Error),
}

impl WindowsCapture {
    pub fn new() -> Self {
        Self {
            control_tx: None,
            event_rx: None,
            width: 0,
            height: 0,
        }
    }
}

impl Capture for WindowsCapture {
    fn enumerate_outputs(&self) -> Result<Vec<OutputInfo>> {
        let (device, _) = dx::create_device()?;
        let dxgi_device: IDXGIDevice2 = device.cast()?;
        let parent: IDXGIAdapter = unsafe { dxgi_device.GetParent() }?;

        let mut outputs = Vec::new();
        let mut i = 0;

        loop {
            let output = match unsafe { parent.EnumOutputs(i) } {
                Ok(o) => o,
                Err(_) => break,
            };

            let output1: IDXGIOutput1 = output.cast()?;
            let desc: DXGI_OUTPUT_DESC = unsafe { output1.GetDesc() }?;

            let output_info = OutputInfo {
                id: i,
                name: String::from_utf16_lossy(&desc.DeviceName)
                    .trim_end_matches('\0')
                    .to_string(),
                width: desc
                    .DesktopCoordinates
                    .right
                    .abs_diff(desc.DesktopCoordinates.left),
                height: desc
                    .DesktopCoordinates
                    .bottom
                    .abs_diff(desc.DesktopCoordinates.top),
                is_primary: desc.AttachedToDesktop.as_bool(),
            };

            outputs.push(output_info);
            i += 1;
        }

        Ok(outputs)
    }

    fn start(&mut self, config: CaptureConfig) -> Result<()> {
        let (control_tx, control_rx) = mpsc::channel::<CaptureControl>(1);
        let (event_tx, event_rx) = mpsc::channel::<CaptureEvent>(ARBITRARY_MEDIA_CHANNEL_LIMIT);

        let output_id = config.output_id.unwrap_or(0);

        std::thread::spawn(move || {
            if let Err(err) = run_desktop_duplication(output_id, control_rx, event_tx.clone()) {
                let _ = event_tx.blocking_send(CaptureEvent::Error(err));
            }
        });

        self.control_tx = Some(control_tx);
        self.event_rx = Some(event_rx);
        tracing::info!("Started Windows DXGI desktop duplication");
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.control_tx.take() {
            let _ = tx.blocking_send(CaptureControl::Stop);
        }
        self.event_rx = None;
        self.width = 0;
        self.height = 0;
        tracing::info!("Stopped Windows DXGI desktop duplication");
        Ok(())
    }

    fn next_frame(&mut self, _timeout: Duration) -> Result<Option<CapturedFrame>> {
        let event_rx = match &mut self.event_rx {
            Some(rx) => rx,
            None => return Ok(None),
        };

        let result = event_rx.blocking_recv();

        match result {
            Some(CaptureEvent::Frame(texture, timestamp)) => {
                let (device, context) = dx::create_device()?;
                let desc = texture.desc();
                self.width = desc.Width;
                self.height = desc.Height;

                let staging = dx::TextureBuilder::new(
                    &device,
                    desc.Width,
                    desc.Height,
                    dx::TextureFormat::BGRA,
                )
                .cpu_access(dx::TextureCPUAccess::Read)
                .usage(dx::TextureUsage::Staging)
                .build()?;

                dx::copy_texture(&staging, &texture, None)?;

                let mut data = Vec::new();
                let mut row_pitch = 0usize;

                staging.map(&context, |d, rp| {
                    data = d.to_vec();
                    row_pitch = rp;
                    Ok(())
                })?;

                Ok(Some(CapturedFrame {
                    frame: VideoFrame::Cpu(crate::types::CpuBuffer {
                        data,
                        width: desc.Width,
                        height: desc.Height,
                        format: crate::types::PixelFormat::Bgra,
                        stride: vec![row_pitch as u32],
                    }),
                    timestamp: timestamp.duration(),
                    duration: Duration::from_secs_f64(1.0 / 60.0),
                }))
            }
            Some(CaptureEvent::Error(err)) => Err(err),
            None => Ok(None),
        }
    }
}

impl Default for WindowsCapture {
    fn default() -> Self {
        Self::new()
    }
}

fn run_desktop_duplication(
    output_index: u32,
    mut control_rx: mpsc::Receiver<CaptureControl>,
    event_tx: mpsc::Sender<CaptureEvent>,
) -> Result<()> {
    let (device, _context) = dx::create_device()?;
    let dxgi_device: IDXGIDevice2 = device.cast()?;
    let parent: IDXGIAdapter = unsafe { dxgi_device.GetParent() }?;

    let output = unsafe { parent.EnumOutputs(output_index) }?;
    let output: IDXGIOutput1 = output.cast()?;

    let duplicated = unsafe { output.DuplicateOutput(&device) }?;

    let desc = unsafe { duplicated.GetDesc() };
    let width = desc.ModeDesc.Width;
    let height = desc.ModeDesc.Height;

    tracing::info!("Desktop duplication started: {}x{}", width, height);

    let texture_pool = TexturePool::new(
        || {
            dx::TextureBuilder::new(&device, width, height, dx::TextureFormat::BGRA)
                .nt_handle()
                .keyed_mutex()
                .build()
                .unwrap()
        },
        10,
    );

    let start_time = std::time::Instant::now();
    let mut last_frame_time = std::time::Instant::now();
    let min_frame_duration = Duration::from_secs_f64(1.0 / 120.0);

    loop {
        match control_rx.try_recv() {
            Ok(CaptureControl::Stop) => {
                tracing::debug!("Received stop signal");
                break;
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => {
                tracing::debug!("Control channel disconnected");
                break;
            }
        }

        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut frame_resource: Option<IDXGIResource> = None;

        unsafe {
            let _ = duplicated.ReleaseFrame();
        }

        let frame_timeout = 100;

        match unsafe {
            duplicated.AcquireNextFrame(frame_timeout, &mut frame_info, &mut frame_resource)
        } {
            Ok(_) => {
                if frame_info.AccumulatedFrames == 0 || frame_info.LastPresentTime == 0 {
                    continue;
                }

                let elapsed = last_frame_time.elapsed();
                if elapsed < min_frame_duration {
                    std::thread::sleep(min_frame_duration - elapsed);
                }

                let Some(frame_resource) = frame_resource else {
                    continue;
                };

                let duplication_texture: ID3D11Texture2D = frame_resource.cast()?;
                let out_texture = texture_pool.acquire();

                {
                    let mut out_desc = D3D11_TEXTURE2D_DESC::default();
                    unsafe {
                        out_texture.GetDesc(&mut out_desc);
                    }

                    let keyed_out = if D3D11_RESOURCE_MISC_FLAG(out_desc.MiscFlags as i32)
                        .contains(D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX)
                    {
                        let keyed: IDXGIKeyedMutex = out_texture.cast()?;
                        unsafe {
                            keyed.AcquireSync(0, u32::MAX)?;
                        }
                        Some(keyed)
                    } else {
                        None
                    };

                    scopeguard::defer! {
                        if let Some(keyed) = keyed_out {
                            unsafe {
                                let _ = keyed.ReleaseSync(0);
                            }
                        }
                    }

                    let device = unsafe { duplication_texture.GetDevice() }?;
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
                            &*out_texture,
                            0,
                            0,
                            0,
                            0,
                            &duplication_texture,
                            0,
                            Some(&region),
                        )
                    };
                }

                let timestamp = Timestamp::new_diff_instant(start_time, std::time::Instant::now());

                match event_tx.try_send(CaptureEvent::Frame(out_texture, timestamp)) {
                    Ok(_) => {
                        last_frame_time = std::time::Instant::now();
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        tracing::debug!("Event channel closed");
                        break;
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::trace!("Frame dropped - channel full");
                    }
                }
            }
            Err(err) => match err.code() {
                DXGI_ERROR_WAIT_TIMEOUT => {
                    continue;
                }
                DXGI_ERROR_ACCESS_LOST => {
                    tracing::debug!("Access lost, stopping capture");
                    break;
                }
                DXGI_ERROR_INVALID_CALL => {
                    tracing::error!("Invalid call in desktop duplication");
                    break;
                }
                DXGI_ERROR_ACCESS_DENIED | E_ACCESSDENIED => {
                    tracing::warn!("Access denied - screen may be locked");
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
                _ => {
                    tracing::error!("Unknown error in desktop duplication: {}", err);
                    break;
                }
            },
        }
    }

    unsafe {
        let _ = duplicated.ReleaseFrame();
    }

    tracing::debug!("Desktop duplication loop exited");
    Ok(())
}
