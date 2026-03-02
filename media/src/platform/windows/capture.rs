use std::time::Duration;

use anyhow::Result;
use windows::{
    core::Interface,
    Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice2, IDXGIOutput1, DXGI_OUTPUT_DESC},
};

use crate::dx;
use crate::traits::Capture;
use crate::types::{CaptureConfig, CapturedFrame, OutputInfo};

pub struct WindowsCapture;

impl WindowsCapture {
    pub fn new() -> Self {
        Self
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
                is_primary: i == 0,
            };

            outputs.push(output_info);
            i += 1;
        }

        Ok(outputs)
    }

    fn start(&mut self, _config: CaptureConfig) -> Result<()> {
        tracing::info!("Starting Windows DXGI desktop duplication");
        tracing::warn!("WindowsCapture::start() is not fully implemented yet");
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    fn next_frame(&mut self, _timeout: Duration) -> Result<Option<CapturedFrame>> {
        Ok(None)
    }
}

impl Default for WindowsCapture {
    fn default() -> Self {
        Self::new()
    }
}
